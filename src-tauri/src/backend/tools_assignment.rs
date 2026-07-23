use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use tauri::State;

use super::project_skill::validate_profile;
use super::user_mcp_config::{mask_env_for_list, merged_servers, UserMcpServer};
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

/// P5 injection reader. Like [`tools_assignment_list_impl`] but DISTINGUISHES an absent
/// `tools.json` (returns `Ok(None)`) from a present file (returns `Ok(Some(list))`, even an
/// empty list). The injection resolver needs this distinction because the per-profile DEFAULT
/// for a MISSING assignment differs by profile (a main coder defaults to its current "inject
/// every merged server" behavior; a mini tier defaults to none) — collapsing absent→`[]` (as
/// [`tools_assignment_list_impl`] does for its callers) would silently strip a main coder's tools.
/// `mini-small` STILL returns `Ok(Some(vec![]))` unconditionally (edits-only, defense-in-depth).
pub fn tools_assignment_opt_impl(working_folder_path: &str, profile: &str) -> Result<Option<Vec<String>>, String> {
    validate_profile(profile)?;
    if profile == "mini-small" {
        // Edits-only tier: an explicit empty assignment regardless of any on-disk file, so the
        // resolver yields [] and never falls through to a profile default.
        return Ok(Some(vec![]));
    }
    let canonical = canonical_working_folder(working_folder_path)?;
    let path = tools_file(&canonical, profile);
    match std::fs::read_to_string(&path) {
        Ok(contents) => match serde_json::from_str::<ToolsAssignment>(&contents) {
            Ok(assignment) => Ok(Some(assignment.assigned)),
            Err(e) => Err(format!("failed to parse tools assignment file: {e}")),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("failed to read tools assignment file: {e}")),
    }
}

/// PURE P5 injection resolver — the single source of truth for WHICH user MCP servers a launch
/// injects for a profile. Inputs:
/// - `assigned`: the per-profile assignment, `None` = no `tools.json` on disk (apply the
///   profile DEFAULT), `Some(list)` = an explicit assignment (even empty).
/// - `available`: the live catalog names, which MUST already be the output of
///   [`merged_servers`] (so they are §9-allowlist-gated, enabled-filtered, reserved-name-safe).
/// - `profile`: the launch profile (one of [`ASSIGNMENT_PROFILES`]).
///
/// Returns the EXACT server names to inject:
/// - `mini-small` → ALWAYS `[]` (edits-only; a hard double-check independent of `assigned`).
/// - `Some(list)` → `list ∩ available`, order-preserving (assignment order) + deduped. A stored
///   name whose server was deleted/disabled/de-allowlisted (absent from `available`) is dropped.
/// - `None` (absent file) → the profile DEFAULT:
///     * a MINI tier (`mini-*`) → `[]` (preserves the historic mini-exclusion invariant: a mini
///       got NO user MCP until the user explicitly assigns one).
///     * a MAIN profile (coder/orchestrator/design) → ALL of `available` (preserves the
///       pre-assignment behavior: the main coder launch injected every merged server). This is
///       what keeps an existing project that never opened the Work Console byte-identical.
pub fn resolve_injected_tools(assigned: Option<&[String]>, available: &[String], profile: &str) -> Vec<String> {
    if profile == "mini-small" {
        return Vec::new();
    }
    match assigned {
        Some(list) => {
            let mut seen = std::collections::HashSet::new();
            list.iter()
                .filter(|name| available.iter().any(|a| a == *name))
                .filter(|name| seen.insert((*name).clone()))
                .cloned()
                .collect()
        }
        None => {
            if profile.starts_with("mini") {
                Vec::new()
            } else {
                available.to_vec()
            }
        }
    }
}

/// P5 launch glue: narrow a launch's merged user MCP servers to the per-profile assignment.
/// `servers` MUST be the output of [`merged_servers`] (already §9-allowlist-gated + enabled-
/// filtered + reserved-name-safe), so this only ever REMOVES servers — it can never add or
/// un-gate one. Reads the profile's `tools.json` (absent vs present, via
/// [`tools_assignment_opt_impl`]) and applies [`resolve_injected_tools`], then retains the matching
/// servers PRESERVING the input order.
///
/// FAIL-OPEN to the profile DEFAULT: an unreadable/malformed assignment file is treated as ABSENT
/// (a broken file must never block a launch), which for a MAIN profile means "inject all merged
/// servers" — byte-identical to the pre-assignment behavior.
pub fn inject_servers_for_profile(
    working_folder_path: &str,
    profile: &str,
    servers: Vec<UserMcpServer>,
) -> Vec<UserMcpServer> {
    // Precondition: only a valid ASSIGNMENT_PROFILE may narrow. An unknown profile is a
    // programming error (the live callers pass fixed literals) — FAIL CLOSED to NO servers
    // rather than risk handing a future, unwired role the entire user-MCP set.
    if validate_profile(profile).is_err() {
        return Vec::new();
    }
    let available: Vec<String> = servers.iter().map(|s| s.name.clone()).collect();
    // Malformed/unreadable ⇒ None (absent semantics) so the profile default applies fail-open.
    let assigned = tools_assignment_opt_impl(working_folder_path, profile).unwrap_or(None);
    let keep = resolve_injected_tools(assigned.as_deref(), &available, profile);
    let keep_set: std::collections::HashSet<&str> = keep.iter().map(String::as_str).collect();
    servers
        .into_iter()
        .filter(|s| keep_set.contains(s.name.as_str()))
        .collect()
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
    // Mask env VALUES for IPC (same as user_mcp_list). Spawn still uses
    // merged_servers / orchestrator_env_json with raw values.
    Ok(mask_servers_env_for_ipc(merged_servers(&app, &canonical)))
}

/// Apply [`mask_env_for_list`] to every server — pure helper so IPC list paths
/// share one mask and unit tests can assert the contract without an AppHandle.
fn mask_servers_env_for_ipc(mut servers: Vec<UserMcpServer>) -> Vec<UserMcpServer> {
    for server in &mut servers {
        server.env = mask_env_for_list(&server.env);
    }
    servers
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

    // ---- P5: tools_assignment_opt_impl (absent vs present distinction) ----

    #[test]
    fn test_opt_absent_is_none() {
        let dir = fresh_dir("opt_absent");
        assert_eq!(tools_assignment_opt_impl(dir.to_str().unwrap(), "coder").unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_opt_present_is_some_even_when_empty() {
        let dir = fresh_dir("opt_present_empty");
        let available = vec!["fs".to_string()];
        tools_assignment_set_impl(dir.to_str().unwrap(), "coder", vec![], &available).unwrap();
        assert_eq!(tools_assignment_opt_impl(dir.to_str().unwrap(), "coder").unwrap(), Some(vec![]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_opt_present_returns_assigned() {
        let dir = fresh_dir("opt_present");
        let available = vec!["fs".to_string(), "git".to_string()];
        tools_assignment_set_impl(dir.to_str().unwrap(), "coder", vec!["fs".to_string(), "git".to_string()], &available).unwrap();
        assert_eq!(
            tools_assignment_opt_impl(dir.to_str().unwrap(), "coder").unwrap(),
            Some(vec!["fs".to_string(), "git".to_string()])
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_opt_mini_small_is_some_empty_even_with_stale_file() {
        // mini-small must report an EXPLICIT empty assignment (Some([])), never None — so the
        // resolver yields [] and never falls through to a (nonexistent) mini default.
        let dir = fresh_dir("opt_mini_small_stale");
        let canon = std::fs::canonicalize(&dir).unwrap();
        let p = canon.join(".claude/tools/mini-small");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("tools.json"), r#"{"assigned":["fs"]}"#).unwrap();
        assert_eq!(tools_assignment_opt_impl(dir.to_str().unwrap(), "mini-small").unwrap(), Some(vec![]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- P5: resolve_injected_tools (the pure injection gate) ----

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn resolve_mini_small_always_empty() {
        // Even with an explicit non-empty assignment whose servers ARE available, mini-small gets none.
        let avail = names(&["fs", "git"]);
        let assigned = names(&["fs", "git"]);
        assert!(resolve_injected_tools(Some(&assigned), &avail, "mini-small").is_empty());
        // And with no file either.
        assert!(resolve_injected_tools(None, &avail, "mini-small").is_empty());
    }

    #[test]
    fn resolve_main_absent_defaults_to_all_available() {
        // A coder with no tools.json keeps the pre-assignment behavior: inject EVERY merged server.
        let avail = names(&["fs", "git", "web"]);
        assert_eq!(resolve_injected_tools(None, &avail, "coder"), avail);
    }

    #[test]
    fn resolve_mini_big_absent_defaults_to_none() {
        // A mini tier with no tools.json keeps the historic mini-exclusion: NO user MCP.
        let avail = names(&["fs", "git"]);
        assert!(resolve_injected_tools(None, &avail, "mini-big").is_empty());
    }

    #[test]
    fn resolve_present_intersects_with_available() {
        // Assigned ∩ available; a server that is no longer in the catalog (deleted/disabled/
        // de-allowlisted) is dropped.
        let avail = names(&["fs", "git"]);
        let assigned = names(&["fs", "deleted", "git"]);
        assert_eq!(resolve_injected_tools(Some(&assigned), &avail, "coder"), names(&["fs", "git"]));
    }

    #[test]
    fn resolve_present_empty_yields_empty_for_main() {
        // An explicit empty assignment is the user's choice: inject nothing (NOT the absent default).
        let avail = names(&["fs", "git"]);
        let assigned: Vec<String> = vec![];
        assert!(resolve_injected_tools(Some(&assigned), &avail, "coder").is_empty());
    }

    #[test]
    fn resolve_preserves_assignment_order_and_dedupes() {
        let avail = names(&["a", "b", "c"]);
        let assigned = names(&["b", "a", "b", "c"]);
        assert_eq!(resolve_injected_tools(Some(&assigned), &avail, "coder"), names(&["b", "a", "c"]));
    }

    #[test]
    fn resolve_mini_big_present_intersects() {
        // Once a mini-big is explicitly assigned, it gets exactly assigned ∩ available.
        let avail = names(&["fs", "git"]);
        let assigned = names(&["git"]);
        assert_eq!(resolve_injected_tools(Some(&assigned), &avail, "mini-big"), names(&["git"]));
    }

    // ---- P5: inject_servers_for_profile (launch glue over real files) ----

    fn server(name: &str) -> UserMcpServer {
        UserMcpServer {
            name: name.to_string(),
            transport: "stdio".to_string(),
            command: "python".to_string(),
            args: vec![],
            env: Default::default(),
            enabled: true,
        }
    }

    #[test]
    fn inject_absent_coder_keeps_all_servers() {
        // No tools.json for coder ⇒ inject every merged server (byte-identical to pre-assignment).
        let dir = fresh_dir("inject_absent");
        let servers = vec![server("fs"), server("git")];
        let got = inject_servers_for_profile(dir.to_str().unwrap(), "coder", servers);
        assert_eq!(got.iter().map(|s| s.name.clone()).collect::<Vec<_>>(), names(&["fs", "git"]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn inject_present_coder_narrows_to_assignment() {
        let dir = fresh_dir("inject_present");
        let available = names(&["fs", "git", "web"]);
        tools_assignment_set_impl(dir.to_str().unwrap(), "coder", names(&["git"]), &available).unwrap();
        let servers = vec![server("fs"), server("git"), server("web")];
        let got = inject_servers_for_profile(dir.to_str().unwrap(), "coder", servers);
        assert_eq!(got.iter().map(|s| s.name.clone()).collect::<Vec<_>>(), names(&["git"]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn inject_mini_small_gets_zero_even_with_servers() {
        // Double-check at the injection seam: mini-small never receives a server.
        let dir = fresh_dir("inject_mini_small");
        let servers = vec![server("fs"), server("git")];
        let got = inject_servers_for_profile(dir.to_str().unwrap(), "mini-small", servers);
        assert!(got.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn inject_invalid_profile_fails_closed_to_zero() {
        // An unknown profile (programming error) must FAIL CLOSED — never hand a role the
        // whole user-MCP set.
        let dir = fresh_dir("inject_bad_profile");
        let servers = vec![server("fs"), server("git")];
        let got = inject_servers_for_profile(dir.to_str().unwrap(), "verifier", servers);
        assert!(got.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn inject_mini_big_absent_gets_zero() {
        // Mini-exclusion preserved: a mini-big with no assignment gets no user MCP.
        let dir = fresh_dir("inject_mini_big_absent");
        let servers = vec![server("fs")];
        let got = inject_servers_for_profile(dir.to_str().unwrap(), "mini-big", servers);
        assert!(got.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// tools_library_list must mask env VALUES for IPC; spawn path (raw merged
    /// servers / orchestrator payload) must keep the real values.
    #[test]
    fn tools_library_list_masks_env_spawn_path_keeps_raw() {
        let mut srv = server("secretsrv");
        srv.env.insert("API_TOKEN".to_string(), "super-secret-value".to_string());
        srv.env.insert("EMPTY".to_string(), String::new());
        let raw = vec![srv];

        // IPC path (what tools_library_list returns after merge).
        let listed = mask_servers_env_for_ipc(raw.clone());
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed[0].env.get("API_TOKEN").map(String::as_str),
            Some("********")
        );
        assert_eq!(
            listed[0].env.get("EMPTY").map(String::as_str),
            Some(""),
            "empty values stay empty"
        );
        let list_json = serde_json::to_string(&listed).unwrap();
        assert!(
            !list_json.contains("super-secret-value"),
            "tools_library_list IPC JSON leaked raw env: {list_json}"
        );

        // Spawn path: raw servers (merged_servers) still carry the secret.
        assert_eq!(
            raw[0].env.get("API_TOKEN").map(String::as_str),
            Some("super-secret-value")
        );
        let orch = super::super::user_mcp_config::orchestrator_env_json(&raw);
        assert!(
            orch.contains("super-secret-value"),
            "orchestrator spawn payload must keep raw env values"
        );
    }
}
