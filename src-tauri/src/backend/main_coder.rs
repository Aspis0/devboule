//! Main coder: Tauri command that appends a first-class Main coder directive to the
//! shared agent ledger.
//!
//! This is the UI twin of the Python MCP tool `spawn_main_coder` (aspis_mcp.py).
//! It validates the request, builds a `MiniCoderDirective` with `tier: Main` and
//! `write_mode: AgenticIterative`, and appends it under the ledger lock.
//! The executor (mini_coder_executor.rs) claims pending directives on its 1.5s
//! scan and, for `DirectiveTier::Main`, always runs the agentic worker (or fails
//! the directive — never a one-shot downgrade).

use tauri::AppHandle;

use crate::backend::mini_coder::{DirectiveTier, MiniCoderDirective, MiniCoderStatus, WriteMode};

/// Generate a directive id: a v4 UUID's 32-hex form — the SAME scheme the Python
/// co-writer uses for every MCP-dispatched directive (`uuid.uuid4().hex`), so ids
/// are globally unique across processes and restarts. Directive ids are assumed
/// unique by every claim/steer/result lookup; a weaker timestamp+counter scheme
/// (the first draft) could collide across restarts (hostile-review finding).
fn generate_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Pure validation for a main-coder request. Returns the cleaned (task, files)
/// tuple, or an error string explaining the first failure encountered.
///
/// This is extracted as a pure helper so the Tauri command can stay thin and
/// the validation logic is directly testable without an AppHandle.
pub(crate) fn validate_main_coder_request(
    task: &str,
    files: &[String],
) -> Result<(String, Vec<String>), String> {
    // 1. Task: non-empty, capped at 4000 chars.
    let task = task.trim().to_string();
    if task.is_empty() {
        return Err("task must not be empty".into());
    }
    if task.len() > 4000 {
        return Err("task exceeds 4000 character limit".into());
    }

    // 2. Files: 1..=10 entries, each must be a project-relative safe path.
    // CO-WRITER PARITY: `MAIN_CODER_MAX_FILES = 10` in oracle/server/aspis_mcp.py
    // (dispatch_spawn_main_coder) enforces the same cap on the MCP path — change
    // BOTH together (Python side pinned by
    // test_spawn_main_coder_caps_files_at_the_rust_twin_limit).
    if files.is_empty() {
        return Err("files must contain at least 1 entry".into());
    }
    if files.len() > 10 {
        return Err("files must contain at most 10 entries".into());
    }

    let validated_files: Vec<String> = files
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let trimmed = f.trim();
            if trimmed.is_empty() {
                Err(format!("files entry {i} is empty after trimming"))
            } else if trimmed.starts_with('/') {
                Err(format!(
                    "files entry {i} must not start with '/': '{trimmed}'"
                ))
            } else if trimmed.starts_with('-') {
                Err(format!(
                    "files entry {i} must not start with '-': '{trimmed}'"
                ))
            } else if trimmed.contains("..") {
                Err(format!(
                    "files entry {i} must not contain '..': '{trimmed}'"
                ))
            } else if trimmed.contains('\\') {
                Err(format!(
                    "files entry {i} must not contain backslashes: '{trimmed}'"
                ))
            } else {
                Ok(trimmed.to_string())
            }
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok((task, validated_files))
}

/// Tauri command: append a first-class Main coder directive to the shared
/// agent ledger.
///
/// This is the UI twin of the Python MCP tool `spawn_main_coder` (aspis_mcp.py).
/// It validates the request, builds a `MiniCoderDirective` with `tier: Main` and
/// `write_mode: AgenticIterative`, and appends it under the ledger lock.
/// The executor (mini_coder_executor.rs) claims pending directives on its 1.5s
/// scan and, for `DirectiveTier::Main`, always runs the agentic worker (or fails
/// the directive — never a one-shot downgrade).
#[tauri::command]
pub fn spawn_main_coder_directive(
    app: AppHandle,
    state: tauri::State<'_, crate::backend::state::BackendState>,
    project_id: String,
    task: String,
    files: Vec<String>,
) -> Result<String, String> {
    // 1. Unlocked vault check (spawning work requires the unlocked vault).
    state.ensure_unlocked()?;

    // 2. Pure validation (see module doc comment).
    let (task, files) = validate_main_coder_request(&task, &files)?;

    // 3. Fail-fast: project must exist (the executor re-resolves the root).
    crate::backend::projects::resolve_project_root_by_id(&app, &project_id)?;

    // 4. Build the directive.
    //
    // MiniCoderDirective does NOT implement Default. Every field must be
    // filled explicitly — the pattern is copied from the `directive(...)`
    // test fixture in mini_coder.rs.
    let id = generate_id();
    let directive = MiniCoderDirective {
        id: id.clone(),
        parent_agent_id: "app-user".into(),
        status: MiniCoderStatus::Pending,
        task,
        files,
        write: true,
        write_mode: WriteMode::AgenticIterative,
        tier: DirectiveTier::Main,
        // Explicit scope: app-authored directives carry their project directly
        // (there is no live parent session to derive it from).
        project_id: Some(project_id.clone()),
        backend: None,
        allow_oracle: false,
        kill_requested: false,
        steer_queue: Vec::new(),
        result_path: format!("{id}.json"),
        agent_id: None,
        created_at: chrono::Utc::now().to_rfc3339(),
        claimed_at: None,
        scratch_path: None,
        started_at: None,
        result: None,
        attempt: 0,
        parent_directive_id: None,
        pigeon_ticket: None,
    };

    // 5. Append under the ledger lock, then run the shared eviction pass so the
    //    directive queue honors MAX_DIRECTIVES like every other mutation site
    //    (terminal directives beyond the cap are evicted oldest-first; an active
    //    one is never dropped).
    crate::backend::agents::mutate_agent_live_state(&app, |st| {
        st.mini_coder_directives.push(directive.clone());
        crate::backend::mini_coder_executor::cap_pass(st);
    })?;

    // 6. Return the directive id.
    Ok(id)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_empty_task_rejected() {
        let err = validate_main_coder_request("", &[]).unwrap_err();
        assert!(err.contains("task must not be empty"));
    }

    #[test]
    fn validate_whitespace_only_task_rejected() {
        let err = validate_main_coder_request("   ", &[]).unwrap_err();
        assert!(err.contains("task must not be empty"));
    }

    #[test]
    fn validate_task_over_4000_chars_rejected() {
        let task = "x".repeat(4001);
        let err = validate_main_coder_request(&task, &[]).unwrap_err();
        assert!(err.contains("4000"));
    }

    #[test]
    fn validate_task_at_4000_chars_accepted() {
        let task = "x".repeat(4000);
        let files = vec![String::from("src/lib.rs")];
        let result = validate_main_coder_request(&task, &files).unwrap();
        assert_eq!(result.0, task);
    }

    #[test]
    fn validate_zero_files_rejected() {
        let err = validate_main_coder_request("task", &[]).unwrap_err();
        assert!(err.contains("at least 1"));
    }

    #[test]
    fn validate_eleven_files_rejected() {
        let files: Vec<String> = (0..11).map(|i| format!("file{i}")).collect();
        let err = validate_main_coder_request("task", &files).unwrap_err();
        assert!(err.contains("at most 10"));
    }

    #[test]
    fn validate_absolute_path_rejected() {
        let files = vec![String::from("/absolute/path")];
        let err = validate_main_coder_request("task", &files).unwrap_err();
        assert!(err.contains("must not start with '/"));
    }

    #[test]
    fn validate_dotdot_path_rejected() {
        let files = vec![String::from("foo/../bar")];
        let err = validate_main_coder_request("task", &files).unwrap_err();
        assert!(err.contains("must not contain '..'"));
    }

    #[test]
    fn validate_dash_leading_path_rejected() {
        let files = vec![String::from("-file.txt")];
        let err = validate_main_coder_request("task", &files).unwrap_err();
        assert!(err.contains("must not start with '-"));
    }

    #[test]
    fn validate_backslash_path_rejected() {
        let files = vec![String::from("foo\\bar")];
        let err = validate_main_coder_request("task", &files).unwrap_err();
        assert!(err.contains("must not contain backslashes"));
    }

    #[test]
    fn validate_empty_file_entry_rejected() {
        let files = vec![String::from("")];
        let err = validate_main_coder_request("task", &files).unwrap_err();
        assert!(err.contains("empty after trimming"));
    }

    #[test]
    fn validate_whitespace_file_entry_rejected() {
        let files = vec![String::from("   ")];
        let err = validate_main_coder_request("task", &files).unwrap_err();
        assert!(err.contains("empty after trimming"));
    }

    #[test]
    fn validate_valid_request_returns_trimmed_values() {
        let files = vec![
            String::from("  src/main.rs  "),
            String::from("  tests/foo.rs  "),
        ];
        let (task, files) = validate_main_coder_request("  hello  ", &files).unwrap();
        assert_eq!(task, "hello");
        assert_eq!(files, vec!["src/main.rs", "tests/foo.rs"]);
    }

    #[test]
    fn validate_single_valid_file_accepted() {
        let files = vec![String::from("src/lib.rs")];
        let (task, files) = validate_main_coder_request("task", &files).unwrap();
        assert_eq!(task, "task");
        assert_eq!(files, vec!["src/lib.rs"]);
    }

    #[test]
    fn validate_ten_files_accepted() {
        let files: Vec<String> = (0..10).map(|i| format!("file{i}")).collect();
        let (task, files) = validate_main_coder_request("task", &files).unwrap();
        assert_eq!(task, "task");
        assert_eq!(files.len(), 10);
    }
}
