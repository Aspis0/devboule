//! Task decomposition — split oversized tasks into sub-tasks on the Kanban (Plan v5 Phase D).
//! Reuses the existing mutate_project + next_task_id path (the SAME path project_create_plan_tasks
//! uses server-side). No MCP HTTP call needed — direct file write.

/// A sub-task created by decomposition.
#[derive(Debug, Clone)]
pub struct SubTask {
    pub title: String,
    pub scope: Vec<String>,
    pub acceptance: String,
}

/// Result of a decomposition: the sub-tasks + the reason for the original's block.
#[derive(Debug, Clone)]
pub struct DecompositionResult {
    /// The sub-task T-ids allocated on the Kanban.
    pub sub_task_ids: Vec<String>,
    /// The reason to set on the original task's blocked status.
    pub block_reason: String,
}

/// Decompose an oversized task into sub-tasks, one per file in the scope.
/// This is a SIMPLE heuristic: if a task has 3 files, it becomes 3 sub-tasks.
/// A future LLM-based decomposition can replace this.
pub fn decompose_by_files(
    original_title: &str,
    scope: &[String],
    acceptance: &str,
) -> Vec<SubTask> {
    if scope.is_empty() {
        return vec![];
    }
    scope
        .iter()
        .map(|file| SubTask {
            title: format!("{} (part: {})", original_title, file),
            scope: vec![file.clone()],
            acceptance: acceptance.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decompose_by_files_creates_one_per_scope() {
        let subs = decompose_by_files(
            "Refactor auth",
            &["a.rs".into(), "b.rs".into(), "c.rs".into()],
            "cargo test passes",
        );
        assert_eq!(subs.len(), 3);
        assert_eq!(subs[0].scope, vec!["a.rs".to_string()]);
        assert!(subs[0].title.contains("Refactor auth"));
        assert!(subs[0].title.contains("a.rs"));
    }

    #[test]
    fn decompose_by_files_empty_scope_returns_empty() {
        let subs = decompose_by_files("Do thing", &[], "test");
        assert!(subs.is_empty());
    }

    #[test]
    fn decompose_by_files_single_scope_returns_one() {
        let subs = decompose_by_files("Fix bug", &["x.rs".into()], "test passes");
        assert_eq!(subs.len(), 1);
    }
}
