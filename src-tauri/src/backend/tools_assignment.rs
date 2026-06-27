use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use tauri::State;

use super::project_skill::validate_profile;
use super::user_mcp_config::{merged_servers, UserMcpServer};
use super::design::{atomic_write, canonical_working_folder, design_write_guard};
use super::state::BackendState;

const MAX_TOOLS_PER_PROFILE: usize = 5;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolsAssignment {
    #[serde(default)]
    pub assigned: Vec<String>,
}

fn tools_file(canonical_root: &Path, profile: &str) -> PathBuf {
    canonical_root.join(".claude/tools").join(profile).join("tools.json")
}

/// Returns the assigned MCP server names for a profile (fail-open empty when the file
/// is absent/unparseable). mini-small is an edits-only tier that can NEVER carry tools,
/// so it returns empty UNCONDITIONALLY here too (defense-in-depth: a hand-edited or
/// pre-constraint `tools.json` must never surface tools for it).
/// P5 PRE-CONDITION: the injection caller MUST intersect this list with the live
/// `merged_servers` catalog before injecting — a stored name whose server was later
/// deleted/disabled must not be injected.
pub fn tools_assignment_list_impl(working_folder_path: &str, profile: &str) -> Result<Vec<String>, String> {
    validate_profile(profile)?;
    if profile == "mini-small" {
        return Ok(vec![]); // edits-only tier — never expose tools, even from a stale file
    }
    let canonical = canonical_working_folder(working_folder_path)?;
    let path = tools_file(&canonical, profile);
    match std::fs::read_to_string(&path) {
        Ok(contents) => {
            match serde_json::from_str::<ToolsAssignment>(&contents) {
                Ok(assignment) => Ok(assignment.assigned),
                Err(e) => Err(format!("failed to parse tools assignment file: {e}")),
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(format!("failed to read tools assignment file: {e}")),
    }
}

pub fn tools_assignment_set_impl(working_folder_path: &str, profile: &str, names: Vec<String>, available: &[String]) -> Result<(), String> {
    validate_profile(profile)?;
    if profile == "mini-small" && !names.is_empty() {
        return Err("the mini-small tier is edits-only and cannot be assigned any tools".into());
    }

    let mut seen = std::collections::HashSet::new();
    let mut deduped = Vec::new();
    for name in names {
        if name.trim().is_empty() {
            return Err("tool names cannot be empty or whitespace".into());
        }
        if seen.insert(name.clone()) {
            deduped.push(name);
        }
    }

    if deduped.len() > MAX_TOOLS_PER_PROFILE {
        return Err(format!("too many tools assigned: maximum is {MAX_TOOLS_PER_PROFILE}, got {}", deduped.len()));
    }

    for name in &deduped {
        if !available.contains(name) {
            return Err(format!("unknown MCP server '{name}' — not in this project's tools"));
        }
    }

    let canonical = canonical_working_folder(working_folder_path)?;
    let path = tools_file(&canonical, profile);
    let json = serde_json::to_string_pretty(&ToolsAssignment { assigned: deduped }).map_err(|e| format!("failed to serialize tools assignment: {e}"))?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| format!("failed to create tools directory: {e}"))?;
    atomic_write(&path, &json, "tools.json")
}

#[tauri::command]
pub fn tools_assignment_list(state: State<'_, BackendState>, working_folder_path: String, profile: String) -> Result<Vec<String>, String> {
    state.ensure_unlocked()?;
    tools_assignment_list_impl(&working_folder_path, &profile)
}

#[tauri::command]
pub fn tools_assignment_set(app: tauri::AppHandle, state: State<'_, BackendState>, working_folder_path: String, profile: String, names: Vec<String>) -> Result<(), String> {
    state.ensure_unlocked()?;
    let _g = design_write_guard()?;
    let canonical = canonical_working_folder(&working_folder_path)?;
    let available: Vec<String> = merged_servers(&app, &canonical).into_iter().map(|s| s.name).collect();
    tools_assignment_set_impl(&working_folder_path, &profile, names, &available)
}

#[tauri::command]
pub fn tools_library_list(app: tauri::AppHandle, state: State<'_, BackendState>, working_folder_path: String) -> Result<Vec<UserMcpServer>, String> {
    state.ensure_unlocked()?;
    let canonical = canonical_working_folder(&working_folder_path)?;
    Ok(merged_servers(&app, &canonical))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process;

    fn fresh_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tools_test_{}_{}", process::id(), tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_round_trip() {
        let dir = fresh_dir("round_trip");
        let available = vec!["fs".to_string(), "git".to_string(), "web".to_string()];
        assert!(tools_assignment_set_impl(dir.to_str().unwrap(), "coder", vec!["fs".to_string(), "git".to_string()], &available).is_ok());
        let result = tools_assignment_list_impl(dir.to_str().unwrap(), "coder").unwrap();
        assert_eq!(result, vec!["fs".to_string(), "git".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_list_no_file() {
        let dir = fresh_dir("no_file");
        let result = tools_assignment_list_impl(dir.to_str().unwrap(), "coder").unwrap();
        assert!(result.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_mini_small_rejects_non_empty() {
        let dir = fresh_dir("mini_small");
        let available = vec!["fs".to_string()];
        assert!(tools_assignment_set_impl(dir.to_str().unwrap(), "mini-small", vec!["fs".to_string()], &available).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_mini_small_empty_ok() {
        let dir = fresh_dir("mini_small_empty");
        let available = vec!["fs".to_string()];
        assert!(tools_assignment_set_impl(dir.to_str().unwrap(), "mini-small", vec![], &available).is_ok());
        let result = tools_assignment_list_impl(dir.to_str().unwrap(), "mini-small").unwrap();
        assert!(result.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_mini_small_list_ignores_a_stale_file() {
        // Defense-in-depth: even if a tools.json exists for mini-small (hand-edited or
        // pre-constraint), the read path must return empty — never surface tools.
        let dir = fresh_dir("mini_small_stale");
        let canon = std::fs::canonicalize(&dir).unwrap();
        let p = canon.join(".claude/tools/mini-small");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("tools.json"), r#"{"assigned":["fs","git"]}"#).unwrap();
        let got = tools_assignment_list_impl(dir.to_str().unwrap(), "mini-small").unwrap();
        assert!(got.is_empty(), "mini-small must never list tools, got {got:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_max_tools_exceeded() {
        let dir = fresh_dir("max_tools");
        let available = vec!["a".to_string(), "b".to_string(), "c".to_string(), "d".to_string(), "e".to_string(), "f".to_string()];
        assert!(tools_assignment_set_impl(dir.to_str().unwrap(), "coder", vec!["a".to_string(), "b".to_string(), "c".to_string(), "d".to_string(), "e".to_string(), "f".to_string()], &available).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_unknown_tool() {
        let dir = fresh_dir("unknown_tool");
        let available = vec!["fs".to_string()];
        assert!(tools_assignment_set_impl(dir.to_str().unwrap(), "coder", vec!["web".to_string()], &available).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_dedupe() {
        let dir = fresh_dir("dedupe");
        let available = vec!["fs".to_string(), "git".to_string()];
        assert!(tools_assignment_set_impl(dir.to_str().unwrap(), "coder", vec!["fs".to_string(), "fs".to_string(), "git".to_string()], &available).is_ok());
        let result = tools_assignment_list_impl(dir.to_str().unwrap(), "coder").unwrap();
        assert_eq!(result, vec!["fs".to_string(), "git".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_bad_profile() {
        let dir = fresh_dir("bad_profile");
        let available = vec!["fs".to_string()];
        assert!(tools_assignment_set_impl(dir.to_str().unwrap(), "mini", vec!["fs".to_string()], &available).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
