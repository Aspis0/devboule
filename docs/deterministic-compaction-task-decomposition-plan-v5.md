# Plan v5 — Deterministic Context & Task Decomposition

**Date:** 2026-07-01
**Status:** Draft (verified against source at `sandbox-epic` HEAD)
**Import:** `@pi-unipi/compactor` (MIT, v2.1.0) + `ambush` (MIT/Apache, v0.1.0)

> **BM25 formula verified 2026-07-01:** checked against `bm25` crate (docs.rs),
> `tldr_core` (code-aware BM25), `vecstore` BM25 implementation, and
> the canonical Okapi BM25 formula. The implementation below matches the
> standard: `score = IDF × tf×(k₁+1)/(tf + k₁×(1−b + b×dl/avgdl))`.
>
> **Workflow verified 2026-07-01** via deep code exploration of the actual
> devboule pipeline (orchestrator → main coder → mini → Censor → reviewer).
> The 5 integration fixes below reconcile the plan with the real structs and
> MCP tools (see §"Reconciliation fixes" at the end of each phase).

---

## ⚠️ Scope

This plan covers **two new capabilities** that are fundamental for local models:

1. **Deterministic context compaction** (BM25, zero-LLM) — compact at **70%** of the model's context window, not just at startup but **dynamically** before every spawn_mini and every burst turn.

2. **Task size estimation + decomposition** — detect oversized tasks before spawning, split them automatically or flag for human.

These are NOT nice-to-have. Without them, local models silently truncate, produce garbage, and the human has no idea why.

---

## 0. How pi handles context windows (and how Devboule mirrors it)

### pi's approach

pi stores `contextWindow` per model in `~/.pi/agent/models.json`:

```json
{
  "providers": {
    "omlx": {
      "models": [
        { "id": "Qwopus3.6-35B-A3B-Coder-4bit", "contextWindow": 262144, "maxTokens": 32768 },
        { "id": "Qwen3.6-27B-OptiQ-4bit",    "contextWindow": 160000, "maxTokens": 32768 },
        { "id": "gemma-4-26B-A4B-it-qat-4bit","contextWindow": 140000, "maxTokens": 32768 }
      ]
    }
  }
}
```

pi reads this at startup and uses `contextWindow` to compute `context_budget`:

- `< 50%` → plenty of room
- `≥ 50%` → warning
- `≥ 75%` → strong warning
- `≥ 90%` → critical
- Auto-compaction threshold is configurable (e.g., 75%).

### Devboule's gap

Devboule's `ModelRegistryEntry` (`model_registry.rs:22`) has `size_bytes`, `tier`,
`thinking_budget` — but **no `context_window`**. The model context window is never
stored, never checked, never used for compaction decisions. Fix: add the field.

---

## 1. Architecture

```
                    ┌──────────────────────────────────────┐
                    │ SETTINGS (models.json mirror)         │
                    │ per-model: contextWindow, maxTokens   │
                    └──────────────┬───────────────────────┘
                                   │ model-specific budget
              ┌────────────────────┼────────────────────┐
              │                    │                     │
    ┌─────────▼──────────┐  ┌─────▼──────────┐  ┌──────▼──────────┐
    │ TAURI BACKEND       │  │ DEVBOULE-CODER  │  │ CLOUD CODER     │
    │                     │  │ (local binary)  │  │ (Claude/Codex)  │
    │ compact before      │  │                 │  │                 │
    │ EVERY spawn_mini:   │  │ compact before  │  │ manages own     │
    │   check usage %     │  │ EVERY burst     │  │ context via     │
    │   if >70% → compact │  │ turn:           │  │ native tools    │
    │                     │  │   check usage % │  │ (no compaction │
    │ also: estimate task │  │   if >70% →     │  │ needed from us)│
    │ size, decompose if  │  │   compact       │  │                 │
    │ too large           │  │                 │  │                 │
    └─────────────────────┘  └─────────────────┘  └─────────────────┘
```

**The compactor lives in TWO places:**

1. `src-tauri/src/backend/compact.rs` — for spawn_mini context
2. `devboule-coder/src/compact.rs` — for burst loop context

Both use the same BM25 algorithm. Both read the model's `context_window` from config.
Both compact at **70%** of that specific model's max.

---

## 2. Phase A: Add `context_window` to model registry

**File:** `src-tauri/src/backend/model_registry.rs`

**Current** (line 22):

```rust
pub struct ModelRegistryEntry {
    pub id: String,
    pub backend: String,
    pub size_bytes: u64,
    pub tier: String,
    pub roles: Vec<String>,
    pub enabled: bool,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub top_k: Option<u32>,
    pub thinking_budget: Option<u32>,
}
```

**Add field:**

```rust
    /// Model context window in tokens (e.g., 262144 for Qwopus 35B, 160000 for Qwen 27B).
    /// Used to compute the 70% compaction threshold. Default: 8192 (safe minimum).
    #[serde(default = "default_context_window")]
    pub context_window: usize,
}

fn default_context_window() -> usize { 8192 }
```

Also add to `DiscoveredModel` (line 184):

```rust
pub struct DiscoveredModel {
    pub id: String,
    pub backend: String,
    pub size_bytes: u64,
    pub param_size: Option<String>,
    pub quant: Option<String>,
    pub recommended_tier: String,
    #[serde(default)]
    pub context_window: Option<usize>,   // NEW — detection hint only
}
```

**Detection (verified 2026-07-01 via web_search):** Neither omlx `/v1/models` nor
ollama `/api/tags` reliably exposes context length. omlx returns only `id`.
Ollama sometimes has it in `details.context_length` (model-dependent). So detection
is BEST-EFFORT: omlx → `None`; ollama → parse `details.context_length` if present.
The curated `ModelRegistryEntry.context_window` (default 8192) is the source of
truth — the user sets it in Settings. This mirrors pi's models.json `contextWindow`,
which is also manually set per model.

### Phase A-UX: Settings UI for `context_window`

The backend field is useless unless the user can read and edit it in Settings.
Three files (the Rust `set_model_registry`/`get_model_registry` commands already
pass the whole entry object, so the field flows over IPC automatically — no new
Tauri command needed):

| File | Change |
|------|--------|
| `src/types/config.ts` | Add `contextWindow?: number;` to `ModelRegistryEntry` (after `thinkingBudget`) |
| `src/components/settings/ModelRegistryCard.tsx` | Add a `Context window` number input next to the `Thinking budget` input (same label/input pattern, placeholder `"8192 default"`, step 1024, min 1024). Wire `onChange` → `updateEntry(backend, id, { contextWindow: parsed })` |
| (no Rust change) | `set_model_registry` already round-trips the full entry; `validate_model_registry` accepts the new field automatically (serde `default`) |

The input shows the curated value (or empty → backend defaults to 8192). When
the user picks a discovered model, `contextWindow` is left empty so the backend
default applies until the user sets it.

---

## 3. Phase B: Deterministic context compactor

### 3.1 The compactor function

**New file:** `src-tauri/src/backend/compact.rs`

```rust
use std::collections::HashMap;

/// BM25 parameters (standard Okapi values).
const BM25_K1: f64 = 1.2;   // term frequency saturation
const BM25_B: f64 = 0.75;   // length normalization

/// Budget report after compaction.
pub struct CompactBudget {
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub percent_saved: f64,
    pub files_kept: usize,
    pub files_trimmed: usize,
    pub findings_kept: usize,
    pub findings_trimmed: usize,
}

/// Block types in a prompt.
enum BlockKind { System, Task, File, Oracle, Findings }

struct PromptBlock {
    kind: BlockKind,
    /// For File blocks: the file path. For others: empty.
    label: String,
    text: String,
}

/// Compact a prompt for a specific model.
///
/// Algorithm:
/// 1. Split prompt into blocks (system, task, file-per-file, oracle, findings)
/// 2. System + task blocks are ALWAYS kept (immutable)
/// 3. File blocks are scored via BM25 against the task description
/// 4. Keep top-N file blocks that fit within the remaining budget
/// 5. Oracle context is truncated to fit remaining budget
/// 6. Censor findings are capped at 5, sorted by severity (High first)
/// 7. Return compacted prompt + budget report
///
/// `model_context_window`: the model's max context in tokens
/// `current_usage`: estimated tokens already in the conversation
/// The budget for THIS prompt = model_context_window * 0.7 - current_usage
pub fn compact_prompt(
    system_prompt: &str,
    task_description: &str,
    files: &[(String, String)],       // (path, content)
    oracle_context: &str,
    censor_findings: &[Finding],
    model_context_window: usize,
    current_usage: usize,
) -> (String, CompactBudget) {
    let budget = (model_context_window * 70 / 100).saturating_sub(current_usage);

    // 1. Tokenize into blocks
    let mut blocks: Vec<PromptBlock> = Vec::new();
    blocks.push(PromptBlock { kind: BlockKind::System, label: String::new(), text: system_prompt.to_string() });
    blocks.push(PromptBlock { kind: BlockKind::Task, label: String::new(), text: task_description.to_string() });
    for (path, content) in files {
        blocks.push(PromptBlock { kind: BlockKind::File, label: path.clone(), text: content.clone() });
    }
    blocks.push(PromptBlock { kind: BlockKind::Oracle, label: String::new(), text: oracle_context.to_string() });
    for f in censor_findings {
        blocks.push(PromptBlock { kind: BlockKind::Findings, label: f.id.clone(), text: format_finding_block(f) });
    }

    let tokens_before: usize = blocks.iter().map(|b| estimate_tokens(&b.text)).sum();

    // 2. Score file blocks via BM25 against the task
    let (file_scores, avg_len, doc_count, term_dfs) = score_files_bm25(task_description, files);

    // 3. Compute how many tokens are taken by system + task (immutable)
    let immutable_tokens: usize = blocks.iter()
        .filter(|b| matches!(b.kind, BlockKind::System | BlockKind::Task))
        .map(|b| estimate_tokens(&b.text))
        .sum();

    let mut remaining = budget.saturating_sub(immutable_tokens);
    let mut file_kept = 0;
    let mut file_trimmed = 0;

    // 4. Sort files by BM25 score, keep top-N that fit in remaining budget
    let mut file_indices: Vec<(usize, f64)> = (0..files.len())
        .map(|i| (i, file_scores.get(i).copied().unwrap_or(0.0)))
        .collect();
    file_indices.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut kept_files: Vec<String> = Vec::new();
    for (idx, _score) in &file_indices {
        let tokens = estimate_tokens(&files[*idx].1);
        if tokens <= remaining || file_kept == 0 {
            kept_files.push(files[*idx].1.clone());
            remaining = remaining.saturating_sub(tokens);
            file_kept += 1;
        } else {
            // Truncate file to fit remaining budget
            let truncated = truncate_to_token_budget(&files[*idx].1, remaining);
            if !truncated.is_empty() {
                kept_files.push(truncated);
                remaining = 0;
                file_kept += 1;
            }
            file_trimmed += 1;
        }
    }
    file_trimmed += files.len().saturating_sub(file_kept);

    // 5. Oracle: truncate to what's left
    let oracle_compact = truncate_to_token_budget(oracle_context, remaining);
    remaining = remaining.saturating_sub(estimate_tokens(&oracle_compact));

    // 6. Findings: sort by severity, cap at 5, keep those that fit
    let mut findings_sorted: Vec<&Finding> = censor_findings.iter().collect();
    findings_sorted.sort_by_key(|f| std::cmp::Reverse(severity_rank(f.severity)));
    let mut finding_kept = 0;
    let mut finding_trimmed = 0;
    let mut findings_text = String::new();
    for f in findings_sorted.iter().take(5) {
        let block = format_finding_block(f);
        let tokens = estimate_tokens(&block);
        if tokens <= remaining || finding_kept == 0 {
            findings_text.push_str(&block);
            findings_text.push('\n');
            remaining = remaining.saturating_sub(tokens);
            finding_kept += 1;
        } else {
            finding_trimmed += 1;
        }
    }
    finding_trimmed += censor_findings.len().saturating_sub(finding_kept + finding_trimmed);

    // 7. Assemble final prompt
    let mut out = String::new();
    out.push_str(system_prompt);
    out.push_str("\n\n");
    out.push_str(task_description);
    out.push_str("\n\n");
    for file_text in &kept_files {
        out.push_str(file_text);
        out.push_str("\n\n");
    }
    out.push_str(&oracle_compact);
    out.push_str("\n\n");
    out.push_str(&findings_text);

    let tokens_after = estimate_tokens(&out);

    (out, CompactBudget {
        tokens_before,
        tokens_after,
        percent_saved: if tokens_before > 0 { (tokens_before - tokens_after) as f64 / tokens_before as f64 * 100.0 } else { 0.0 },
        files_kept: file_kept,
        files_trimmed,
        findings_kept: finding_kept,
        findings_trimmed,
    })
}

/// Rough token estimation: chars / 4 for English text.
fn estimate_tokens(text: &str) -> usize { text.len() / 4 }

/// Truncate text to fit within a token budget, preserving UTF-8 boundaries.
fn truncate_to_token_budget(text: &str, budget_tokens: usize) -> String {
    let max_chars = budget_tokens * 4;
    if text.len() <= max_chars { return text.to_string(); }
    let mut end = max_chars;
    while !text.is_char_boundary(end) { end -= 1; }
    text[..end].to_string()
}

/// Format a single Censor finding as a compact text block.
fn format_finding_block(f: &Finding) -> String {
    format!("[{}] {}: {} ({}:{})",
        match f.severity { Severity::High => "HIGH", Severity::Medium => "MED", Severity::Low => "LOW" },
        f.source, f.title, f.file, f.line.map_or(String::new(), |l| l.to_string()))
}

/// BM25 score each file content against the task description.
/// Returns (scores, avg_doc_len, doc_count, term_dfs).
fn score_files_bm25(query: &str, files: &[(String, String)]) -> (Vec<f64>, f64, usize, HashMap<String, usize>) {
    let query_terms: Vec<String> = query.to_lowercase().split_whitespace().map(String::from).collect();
    if query_terms.is_empty() {
        return (vec![0.0; files.len()], 1.0, files.len(), HashMap::new());
    }

    // Compute document lengths + term document frequencies
    let docs: Vec<Vec<String>> = files.iter()
        .map(|(_, content)| content.to_lowercase().split_whitespace().map(String::from).collect())
        .collect();
    let doc_count = docs.len();
    let avg_len = if doc_count > 0 {
        docs.iter().map(|d| d.len()).sum::<usize>() as f64 / doc_count as f64
    } else {
        1.0
    };
    let avg_len = avg_len.max(1.0);

    // Term DF: how many docs contain each term
    let mut term_dfs: HashMap<String, usize> = HashMap::new();
    for doc in &docs {
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for term in doc {
            if seen.insert(term) {
                *term_dfs.entry(term.clone()).or_insert(0) += 1;
            }
        }
    }

    // BM25 score per document
    let scores: Vec<f64> = docs.iter().map(|doc| {
        let dl = doc.len() as f64;
        query_terms.iter().map(|qt| {
            let tf = doc.iter().filter(|t| *t == qt).count() as f64;
            if tf == 0.0 { return 0.0; }
            let df = *term_dfs.get(qt).unwrap_or(&1) as f64;
            let idf = ((doc_count as f64 - df + 0.5) / (df + 0.5) + 1.0).ln();
            idf * (tf * (BM25_K1 + 1.0)) / (tf + BM25_K1 * (1.0 - BM25_B + BM25_B * dl / avg_len))
        }).sum()
    }).collect();

    (scores, avg_len, doc_count, term_dfs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bm25_scores_relevant_file_higher() {
        let query = "user authentication login";
        let files = vec![
            ("auth.rs".into(), "fn login() { authenticate user password }".into()),
            ("style.css".into(), "body { color: red; margin: 0; }".into()),
        ];
        let (scores, _, _, _) = score_files_bm25(query, &files);
        assert!(scores[0] > scores[1], "auth.rs should score higher than style.css");
    }

    #[test]
    fn compact_respects_budget() {
        let prompt = "x".repeat(100_000);  // huge prompt
        let system = "You are a coder.";
        let task = "Fix bug in auth.";
        let files = vec![("big.rs".into(), prompt.clone())];
        let (compacted, budget) = compact_prompt(system, task, &files, "", &[], 8_192, 0);
        assert!(estimate_tokens(&compacted) <= 8_192 * 70 / 100,
            "compacted prompt must fit within 70% of 8K window");
        assert!(budget.percent_saved > 0.0, "some savings expected");
    }

    #[test]
    fn empty_files_and_findings_works() {
        let (compacted, _) = compact_prompt("sys", "task", &[], "", &[], 32_000, 0);
        assert!(compacted.contains("sys"));
        assert!(compacted.contains("task"));
    }
}
```

### 3.2 Where compaction runs

**Location 1: Tauri backend — before every spawn_mini**

**File:** `src-tauri/src/backend/mini_coder_executor.rs`

> **RECONCILIATION FIX 1+2 (verified 2026-07-01):** `MiniCoderDirective`
> (`mini_coder.rs:435`) has `backend: Option<String>` but **no `model` field**.
> The model is chosen at spawn time from the project's mini backend config.
> And `build_mini_prompt` (`mini_coder_executor.rs:3821`) signature is:
> `fn build_mini_prompt(backend: &MiniCoderBackend, directive: &MiniCoderDirective,
>  project_root: &Path, result_target: &Path, oracle_access: Option<&MiniOracleAccess>) -> String`
> — it takes the directive (which carries `files`, `task`, `write`) and builds
> the prompt internally by reading file contents itself. Compaction CANNOT inject
> a pre-built prompt. **Approach: post-process.** Call `build_mini_prompt`
> normally, then run `compact_prompt` on the returned string, then send.

In the spawn path, after task selection:

```rust
// FIX 1: resolve context window from the directive's BACKEND (no model field)
// Look up the model configured for this backend in the model registry / Settings.
let context_window = resolve_model_context_window_for_backend(&directive.backend);

// Build the prompt normally — build_mini_prompt reads file contents itself
// (it already caps files at FUZZY_MAX_FILE_BYTES=256KB and MAX_PROMPT_FILES=20).
let raw_prompt = build_mini_prompt(&backend, &directive, &project_root, &result_target, &oracle_access);

// FIX 2: post-process the built prompt with BM25 compaction
let current_usage = estimate_current_context_usage(&directive);
let estimated_prompt_tokens = estimate_tokens(&raw_prompt);

let (prompt, budget) = if current_usage + estimated_prompt_tokens > context_window * 70 / 100 {
    let (compacted, budget) = compact::compact_built_prompt(
        &raw_prompt, &directive.task, context_window, current_usage,
    );
    log::info!("compaction spawn: {}→{} tokens ({:.0}% saved, {} files kept, {} findings)",
        budget.tokens_before, budget.tokens_after, budget.percent_saved,
        budget.files_kept, budget.findings_kept);
    (compacted, Some(budget))
} else {
    (raw_prompt, None)
};

// Send the (possibly compacted) prompt to the model
send_to_model(&prompt);
```

The `compact_built_prompt` variant parses the already-built prompt string back
into blocks (system/task/file/oracle/findings by sentinel markers
`build_mini_prompt` already emits) and applies BM25 + budget truncation.
This avoids duplicating `build_mini_prompt`'s file-reading logic.

**Location 2: devboule-coder — before every burst turn**

**File:** `devboule-coder/src/agent_loop.rs`

In the burst loop, before calling the model:

```rust
// Check context usage
let usage = estimate_tokens(&transcript);
let context_window = self.model_context_window;  // from config

if usage > context_window * 70 / 100 {
    let (compacted_transcript, budget) = compact_session(
        &transcript, context_window,
    );
    transcript = compacted_transcript;
    log::info!("compaction burst: {}→{} tokens ({:.0}% saved)",
        budget.tokens_before, budget.tokens_after, budget.percent_saved);
}
```

**Location 3: devboule-coder — on session start/resume**

**File:** `devboule-coder/src/main.rs` or `session.rs`

When loading a previous session, compact it before the first burst turn:

```rust
if let Some(saved_session) = load_session(&session_path) {
    let usage = estimate_tokens(&saved_session);
    if usage > context_window * 70 / 100 {
        let (compacted, _) = compact_session(&saved_session, context_window);
        transcript = compacted;
    }
}
```

### 3.3 Why 70%?

pi's compactor warns at 50%, strongly warns at 75%, and auto-compacts at a
configurable threshold. Devboule's 70% threshold is a safe middle ground:

- **Below 70%:** context is clean, no compaction needed
- **At 70%:** compact BEFORE the next spawn/turn — leaves 30% for model output
- **Output budget:** 30% of window for model generation (for a 160K model, that's 48K tokens of output — more than enough)

The 70% is **per-model**. A Qwen 4B with 8K window compacts at 5.6K tokens.
A Qwopus 35B with 262K window compacts at 183K tokens. Same rule, different
thresholds from the same `context_window` field.

---

## 4. Phase C: Task size estimation + decomposition

### 4.1 `estimate_task_size()`

**New file:** `src-tauri/src/backend/task_size.rs`

```rust
pub struct TaskEstimate {
    pub estimated_input_tokens: usize,      // what the mini will receive
    pub scope_files: usize,
    pub scope_bytes: usize,
    pub fits_model: bool,
    pub reason: Option<String>,
}

pub fn estimate_task_size(
    task_title: &str,
    task_scope: &[String],
    project_root: &Path,
    model_context_window: usize,
) -> TaskEstimate {
    let task_tokens = task_title.len() / 4;
    let system_overhead = 3_000;  // prompt + skill text ~3K tokens
    let oracle_overhead = 2_000;  // project structure ~2K

    let mut scope_tokens = 0;
    let mut scope_bytes = 0;
    for file in task_scope {
        if let Ok(content) = std::fs::read_to_string(project_root.join(file)) {
            scope_tokens += content.len() / 4;
            scope_bytes += content.len();
        }
    }

    let estimated = task_tokens + scope_tokens + system_overhead + oracle_overhead;
    let budget = model_context_window * 70 / 100;
    let fits = estimated <= budget;

    TaskEstimate {
        estimated_input_tokens: estimated,
        scope_files: task_scope.len(),
        scope_bytes,
        fits_model: fits,
        reason: if !fits {
            Some(format!(
                "task needs ~{estimated} tokens ({} files, {} bytes) but model budget is {budget} tokens (70% of {model_context_window}). Split into smaller tasks.",
                task_scope.len(), scope_bytes
            ))
        } else { None },
    }
}
```

### 4.2 Integration with spawn_mini

Called from `mini_coder_executor.rs` BEFORE spawning:

```rust
let estimate = task_size::estimate_task_size(&task.title, &task.scope, &root, context_window);
if !estimate.fits_model {
    // Decompose if possible, or refuse with clear error
    return Err(format!("task too large: {}", estimate.reason.unwrap_or_default()));
}
```

> **RECONCILIATION FIX 4 (verified 2026-07-01):** the runner's `build_spawn_params`
> (`runner.rs:548`) ALREADY caps the task *description* string at
> `MAX_DELEGATED_TASK_CHARS = 4200` (~1050 tokens). That handles task-TEXT bloat.
> This new `estimate_task_size` is complementary — it handles SCOPE-FILE bloat
> (the file contents `build_mini_prompt` reads). The two caps do NOT overlap:
> the existing cap trims `task.title + acceptance`; the new cap trims
> `file contents + oracle context`. No duplication; keep both.

### 4.3 Why this is agnostic

Smaller tasks help **every** model. The threshold changes per model, but the
principle is universal:

| Model | Context | 70% budget | Task that fails | Task that fits |
|-------|---------|-----------|-----------------|----------------|
| Qwen 4B | 8K | 5.6K | "Implement auth" (8 files, 12K tokens) | "Add login form" (1 file, 2K tokens) |
| Qwen 35B | 160K | 112K | "Rewrite entire codebase" | "Refactor auth module" (3 files, 40K tokens) |
| Claude | 200K | 140K | Rarely hits limit | Almost everything fits |

The estimation is the same code path for all models — only the `context_window`
parameter changes.

---

## 5. Phase D: Task decomposition

### When estimation fails

```rust
/// Decompose an oversized task. Tries cloud model first, falls back to human prompt.
pub async fn decompose_task(
    app: &AppHandle,
    task: &ProjectTask,
    project_id: &str,
    context_window: usize,
) -> Result<Vec<String>, String> {
    let budget = context_window * 70 / 100;

    // Build decomposition prompt
    let prompt = format!(
        "Split this task into smaller sub-tasks, each fitting within {budget} tokens:\n\
         Task: {}\nScope: {:?}\n\
         Output: JSON array of {{title, scope, acceptance}} objects.",
        task.title, task.scope
    );

    // Try cloud model for decomposition
    if let Some(cloud) = available_cloud_model(app) {
        let result = cloud.generate(&prompt).await?;
        let sub_tasks: Vec<SubTask> = serde_json::from_str(&result)?;
        let ids = create_sub_tasks_on_kanban(app, project_id, &task.id, &sub_tasks).await?;
        mark_task_blocked(app, project_id, &task.id, "decomposed").await?;
        return Ok(ids);
    }

    // No cloud model — emit event to frontend for human decomposition
    let _ = app.emit("task://needs-decomposition", serde_json::json!({
        "taskId": task.id,
        "title": task.title,
        "estimatedTokens": estimate_task_size(&task.title, &task.scope, &root, context_window)
            .estimated_input_tokens,
        "modelBudget": budget,
    }));
    Err("task requires human decomposition — frontend notified".into())
}
```

---

## 6. Summary

| Phase | File | What | Why |
|-------|------|------|-----|
| A | `model_registry.rs` | Add `context_window` field | Foundation — needed by all compaction decisions |
| B | `compact.rs` (new) | BM25 compactor + `compact_prompt()` | Every spawn_mini, every burst turn, every session start |
| B | `mini_coder_executor.rs` | Hook compaction before spawn | Prevent silent truncation |
| B | `devboule-coder/agent_loop.rs` | Hook compaction before burst | Keep local main coder's context bounded |
| B | `devboule-coder/main.rs` | Compact on session resume | Survive restarts |
| C | `task_size.rs` (new) | `estimate_task_size()` | Refuse to send doomed prompts |
| C | `mini_coder_executor.rs` | Hook estimation before spawn | Same spawn path, before compaction |
| D | `task_decompose.rs` (new) | `decompose_task()` | Auto-split oversized tasks |
| D | `devboule-coder/runner.rs` | Handle "decomposed" result | Runner understands new blocked reason |

**Zero new Rust dependencies.** BM25 is ~60 lines, estimation is arithmetic,
decomposition reuses existing cloud model plumbing.

**Verified:** BM25 formula matches canonical Okapi implementation (confirmed against
`bm25` crate, `tldr_core`, `vecstore`, 2026-07-01).

---

## Phase B.5: Hybrid BM25 + Vector (RRF)

**Why:** pure BM25 is lexical — "login" won't surface "authentication" unless the
word appears. Hybrid search combines BM25 (lexical) with vector (semantic) via
Reciprocal Rank Fusion (RRF), the industry standard (Elasticsearch, Azure AI).

**RRF formula:** `score(d) = 1/(k+bm25_rank) + 1/(k+vector_rank)` where `k=60`.
No score normalization needed (avoids the production failure of weighted averaging).

**Embedding source:** the EXISTING Oracle embedder — `Qwen/Qwen3-Embedding-0.6B`
(1024 dims, sentence-transformers in the Oracle Python process). Already loaded,
loopback, no cloud, no new model to install. Verified at `oracle/config.py:11`.

**New endpoint:** `POST /embed-bounded` on the Oracle HTTP server's `bounded_router`
(agent-token auth via `x-oracle-auth-token`). Calls `embed_texts(texts)` → returns
`{embeddings, dims}`. If the embedder is busy (indexing) or unavailable → HTTP 503,
caller falls back to BM25-only (graceful, no regression).

**Plumbing:**

- Tauri `compact.rs`: `embed_texts_via_oracle(base_url, agent_token, texts)` → `Vec<Vec<f64>>`.
  Has `AppHandle` → reads `OracleHttpSession` base_url + `oracle_agent_token()`.
- devboule-coder `model_client.rs`: same call, via NEW env vars `DEVBOULE_ORACLE_BASE_URL`
  - `DEVBOULE_ORACLE_AGENT_TOKEN` (set by Tauri at launch alongside existing DEVBOULE_*).
- Both compactors: `rrf_fuse(bm25_ranks, vector_ranks, k=60)` → final rank. If embed
  call fails → BM25-only (current behavior, zero regression).

**Graceful fallback is mandatory:** a user without Oracle running gets BM25-only.
No crash, no hang, no dependency on the server being up.
