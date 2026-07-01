//! Task size estimation — detect oversized tasks before spawning a mini (Plan v5 Phase C).
//! Reads scoped files, estimates tokens, compares against the model's context window.

use std::path::Path;

/// Estimate of whether a task fits the assigned model.
#[derive(Debug, Clone)]
pub struct TaskEstimate {
    /// Estimated input tokens (task + files + system overhead).
    pub estimated_input_tokens: usize,
    /// Number of files in the task scope.
    pub scope_files: usize,
    /// Total bytes of scoped files.
    pub scope_bytes: usize,
    /// Does it fit within 70% of the model's context window?
    pub fits_model: bool,
    /// Human-readable reason if it doesn't fit.
    pub reason: Option<String>,
}

/// Measure the ACTUAL prompt overhead: skill + project context files.
/// Falls back to 5K minimum when files are absent.
fn measure_overhead_tokens(project_root: &Path) -> usize {
    let mut tokens = 3_000; // system prompt + constraints + contract (base)
                            // Skill file: .claude/skills/mini/SKILL.md
    let skill = project_root
        .join(".claude")
        .join("skills")
        .join("mini")
        .join("SKILL.md");
    if let Ok(content) = std::fs::read_to_string(&skill) {
        tokens += content.len() / 4;
    }
    // Project context: AGENTS.md or CLAUDE.md
    for ctx_file in ["AGENTS.md", "CLAUDE.md"] {
        if let Ok(content) = std::fs::read_to_string(project_root.join(ctx_file)) {
            tokens += content.len() / 4;
            break; // only one of the two is used
        }
    }
    tokens.max(5_000) // floor: never less than 5K
}

/// Estimate whether a task fits within 70% of the assigned model's context window.
/// Reads scoped files, sums tokens (~4 chars/token), adds system + oracle overhead.
/// Called BEFORE build_mini_prompt — if !fits_model, refuse to spawn.
pub fn estimate_task_size(
    task_title: &str,
    task_scope: &[String],
    project_root: &Path,
    model_context_window: usize,
) -> TaskEstimate {
    let task_tokens = task_title.len() / 4;
    let mut scope_tokens = 0;
    let mut scope_bytes = 0;
    for file in task_scope {
        // Reject path traversal: no .. or absolute paths.
        let path = project_root.join(file);
        let path = match path.canonicalize() {
            Ok(p) => p,
            Err(_) => continue, // file doesn't exist — skip (logged below)
        };
        let root = match project_root.canonicalize() {
            Ok(r) => r,
            Err(_) => continue,
        };
        if !path.starts_with(&root) {
            continue; // outside project — skip
        }
        if let Ok(content) = std::fs::read_to_string(&path) {
            scope_tokens += content.len() / 4;
            scope_bytes += content.len();
        } else {
            eprintln!(
                "task_size: could not read {} (missing or non-UTF8)",
                path.display()
            );
        }
    }
    let estimated = task_tokens + scope_tokens + measure_overhead_tokens(project_root);
    let budget = model_context_window * 70 / 100;
    let fits = estimated <= budget;
    let reason = if !fits {
        Some(format!(
            "task needs ~{estimated} tokens ({} files, {} bytes) but model budget is {budget} tokens (70% of {model_context_window}). Split into smaller tasks.",
            task_scope.len(), scope_bytes
        ))
    } else {
        None
    };
    TaskEstimate {
        estimated_input_tokens: estimated,
        scope_files: task_scope.len(),
        scope_bytes,
        fits_model: fits,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_fits_small_task() {
        let tmp = std::env::temp_dir().join(format!("aspis-tasksize-small-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // Small file: 100 bytes → ~25 tokens. Task + overhead ~5K tokens. Fits 8K window.
        std::fs::write(tmp.join("a.rs"), "fn main() { println!(\"hi\"); }").unwrap();
        let est = estimate_task_size("Fix the bug", &["a.rs".into()], &tmp, 8_192);
        assert!(est.fits_model, "small task fits 8K: {:?}", est.reason);
        assert!(est.estimated_input_tokens > 0);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn estimate_rejects_huge_task() {
        let tmp = std::env::temp_dir().join(format!("aspis-tasksize-huge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // Huge file: 200KB → ~50K tokens. Way over 8K window.
        std::fs::write(tmp.join("big.rs"), "x ".repeat(100_000)).unwrap();
        let est = estimate_task_size("Refactor everything", &["big.rs".into()], &tmp, 8_192);
        assert!(!est.fits_model, "huge task must not fit 8K");
        assert!(est.reason.is_some(), "must have a reason");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
