//! Resolve projects dir and agents state path (Aspis + Devboule env dual-read).

use std::env;
use std::path::{Path, PathBuf};

pub const AGENTS_STATE_FILE: &str = ".aspis-agents.json";

/// Prefer Devboule env names, fall back to Aspis legacy.
///
/// Order:
/// 1. `DEVBOULE_MCP_PROJECTS_DIR`
/// 2. `ASPIS_MCP_PROJECTS_DIR`
/// 3. `ASPIS_PROJECTS_DIR`
/// 4. `{DEVBOULE_MCP_ROOT|ASPIS_MCP_ROOT}/projects`
/// 5. `{cwd}/projects` (last resort for tests)
pub fn resolve_projects_dir() -> PathBuf {
    for key in [
        "DEVBOULE_MCP_PROJECTS_DIR",
        "ASPIS_MCP_PROJECTS_DIR",
        "ASPIS_PROJECTS_DIR",
    ] {
        if let Ok(v) = env::var(key) {
            let t = v.trim();
            if !t.is_empty() {
                return PathBuf::from(t);
            }
        }
    }
    for key in ["DEVBOULE_MCP_ROOT", "ASPIS_MCP_ROOT"] {
        if let Ok(v) = env::var(key) {
            let t = v.trim();
            if !t.is_empty() {
                return PathBuf::from(t).join("projects");
            }
        }
    }
    env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("projects")
}

pub fn agents_state_path(projects_dir: &Path) -> PathBuf {
    projects_dir.join(AGENTS_STATE_FILE)
}

pub fn agents_lock_path(projects_dir: &Path) -> PathBuf {
    projects_dir.join(format!("{AGENTS_STATE_FILE}.lock"))
}
