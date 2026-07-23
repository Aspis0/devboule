//! CKG + project structure tools (P6):
//! `project_structure`, `get_neighborhood`, `find_imports`.
//!
//! # Architecture
//!
//! * `project_structure` shells out to `DEVBOULE_APP_BIN` / `ASPIS_APP_BIN`
//!   `structure --root <path>` (same bridge as Python). Fail-closed when the
//!   binary is unset / non-executable.
//! * CKG tools read `ckg.sqlite` (WAL) with rusqlite — no second graph builder.
//!
//! # Security
//!
//! * Role allowlists + session token via `require_agent_tool`.
//! * Work root via `validate_project_work_root` (path confinement).
//! * Mini cross-project scope rejected.
//! * Neighborhood / imports filtered to in-scope file_ids only (no full-corpus leak).

use crate::project_file::normalize_project_id;
use crate::state::{clean_text, ToolError, ToolResult};
use crate::tools::agent_lifecycle::require_agent_tool;
use crate::tools::oracle::{
    audit_agent_read, enforce_mini_oracle_project_scope, management_root_from_projects_dir,
    oracle_allowed_file_ids, resolve_project_work_root,
};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const APP_BIN_ENVS: &[&str] = &["DEVBOULE_APP_BIN", "ASPIS_APP_BIN"];
const PROJECT_STRUCTURE_MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
/// Wall-clock kill for the structure bridge subprocess (Python PROJECT_STRUCTURE_TIMEOUT_S).
const PROJECT_STRUCTURE_TIMEOUT: Duration = Duration::from_secs(60);
const STRUCTURE_CACHE_TTL: Duration = Duration::from_secs(30);

// ── structure bridge ────────────────────────────────────────────────────────

fn resolve_structure_bridge_binary() -> ToolResult<PathBuf> {
    let raw = APP_BIN_ENVS
        .iter()
        .find_map(|k| std::env::var(k).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let Some(raw) = raw else {
        return Err(ToolError::new(
            "project_structure is unavailable: the app binary path is not configured \
             (DEVBOULE_APP_BIN / ASPIS_APP_BIN unset). Relaunch the agent from the app so the bridge \
             is wired.",
        ));
    };
    let candidate = PathBuf::from(&raw);
    let ok = candidate.is_file()
        && {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                candidate
                    .metadata()
                    .map(|m| m.permissions().mode() & 0o111 != 0)
                    .unwrap_or(false)
            }
            #[cfg(not(unix))]
            {
                true
            }
        };
    if !ok {
        let name = candidate
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("binary");
        return Err(ToolError::new(format!(
            "project_structure is unavailable: configured app binary '{name}' \
             is not an executable file."
        )));
    }
    Ok(candidate)
}

fn run_structure_bridge(app_bin: &Path, root: &Path) -> ToolResult<Value> {
    let mut child = Command::new(app_bin)
        .args(["structure", "--root"])
        .arg(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            ToolError::new(format!(
                "project_structure could not run the structure bridge: {e}"
            ))
        })?;
    // Drain pipes on side threads so a large graph cannot fill the OS pipe buffer
    // and deadlock while we poll try_wait.
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let stdout_h = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut r) = stdout_pipe {
            let _ = r.read_to_end(&mut buf);
        }
        buf
    });
    let stderr_h = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut r) = stderr_pipe {
            let _ = r.read_to_end(&mut buf);
        }
        buf
    });
    let deadline = Instant::now() + PROJECT_STRUCTURE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = stdout_h.join();
                    let _ = stderr_h.join();
                    return Err(ToolError::new(format!(
                        "project_structure timed out after {}s (structure bridge killed).",
                        PROJECT_STRUCTURE_TIMEOUT.as_secs()
                    )));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_h.join();
                let _ = stderr_h.join();
                return Err(ToolError::new(format!(
                    "project_structure could not wait on the structure bridge: {e}"
                )));
            }
        }
    };
    let stdout = stdout_h.join().unwrap_or_default();
    let stderr = stderr_h.join().unwrap_or_default();
    if !status.success() {
        let detail = String::from_utf8_lossy(&stderr);
        let line = detail
            .lines()
            .next()
            .unwrap_or("no diagnostic")
            .chars()
            .take(200)
            .collect::<String>();
        return Err(ToolError::new(format!(
            "project_structure bridge failed (exit {}): {line}",
            status.code().unwrap_or(-1)
        )));
    }
    if stdout.len() > PROJECT_STRUCTURE_MAX_OUTPUT_BYTES {
        return Err(ToolError::new(
            "project_structure graph output exceeded the size limit.",
        ));
    }
    let graph: Value = serde_json::from_slice(&stdout).map_err(|_| {
        ToolError::new("project_structure bridge returned unparseable JSON.")
    })?;
    if !graph.is_object() {
        return Err(ToolError::new(
            "project_structure bridge returned a non-object graph.",
        ));
    }
    Ok(graph)
}

fn structure_summary(graph: &Value) -> Value {
    json!({
        "scanned": graph.get("scanned"),
        "skippedTooLarge": graph.get("skippedTooLarge"),
        "skippedUnsupported": graph.get("skippedUnsupported"),
        "skippedUnreadable": graph.get("skippedUnreadable"),
        "capped": graph.get("capped").and_then(|v| v.as_bool()).unwrap_or(false),
    })
}

fn compact_structure(graph: &Value, full: bool) -> Value {
    let spine = graph
        .get("spine")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let mut result = json!({
        "spine": if spine.is_array() { spine } else { json!([]) },
        "summary": structure_summary(graph),
    });
    if full {
        let files = graph
            .get("files")
            .cloned()
            .unwrap_or_else(|| json!([]));
        result.as_object_mut().unwrap().insert(
            "files".into(),
            if files.is_array() { files } else { json!([]) },
        );
    }
    result
}

struct StructureCacheEntry {
    at: Instant,
    root: String,
    graph: Value,
}

static STRUCTURE_CACHE: Mutex<Option<StructureCacheEntry>> = Mutex::new(None);

fn build_project_structure(work_root: &Path, full: bool) -> ToolResult<Value> {
    let app_bin = resolve_structure_bridge_binary()?;
    let root_str = work_root.to_string_lossy().to_string();
    if let Ok(guard) = STRUCTURE_CACHE.lock() {
        if let Some(entry) = guard.as_ref() {
            if entry.root == root_str && entry.at.elapsed() < STRUCTURE_CACHE_TTL {
                return Ok(compact_structure(&entry.graph, full));
            }
        }
    }
    let graph = run_structure_bridge(&app_bin, work_root)?;
    if let Ok(mut guard) = STRUCTURE_CACHE.lock() {
        *guard = Some(StructureCacheEntry {
            at: Instant::now(),
            root: root_str,
            graph: graph.clone(),
        });
    }
    Ok(compact_structure(&graph, full))
}

pub fn project_structure(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    session_token: Option<&str>,
    project_id: &str,
    full: bool,
) -> ToolResult<Value> {
    let (agent_id, role) = require_agent_tool(
        projects_dir,
        agent_id,
        role,
        "project_structure",
        session_token,
    )?;
    let project_id = normalize_project_id(project_id)?;
    enforce_mini_oracle_project_scope(projects_dir, &agent_id, &role, Some(&project_id))?;
    let work_root = resolve_project_work_root(projects_dir, &project_id)?;
    let payload = build_project_structure(&work_root, full)?;
    let spine_len = payload
        .get("spine")
        .and_then(|s| s.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let _ = audit_agent_read(
        projects_dir,
        &agent_id,
        &role,
        "project_structure",
        &format!("Read project structure spine ({spine_len} files)."),
        Some(&project_id),
    );
    let mut out = json!({
        "projectId": project_id,
        "spine": payload.get("spine").cloned().unwrap_or_else(|| json!([])),
        "summary": payload.get("summary").cloned().unwrap_or_else(|| json!({})),
    });
    if let Some(files) = payload.get("files") {
        out.as_object_mut()
            .unwrap()
            .insert("files".into(), files.clone());
    }
    Ok(out)
}

// ── CKG store (read-only) ───────────────────────────────────────────────────

fn ckg_db_path(projects_dir: &Path) -> PathBuf {
    if let Ok(p) = std::env::var("CKG_DB_PATH") {
        let t = p.trim();
        if !t.is_empty() {
            return PathBuf::from(t);
        }
    }
    if let Ok(dir) = std::env::var("ORACLE_DIR") {
        let t = dir.trim();
        if !t.is_empty() {
            return PathBuf::from(t).join("ckg.sqlite");
        }
    }
    management_root_from_projects_dir(projects_dir)
        .join("oracle-data")
        .join("ckg.sqlite")
}

fn open_ckg_readonly(path: &Path) -> ToolResult<Option<Connection>> {
    if !path.exists() {
        return Ok(None);
    }
    let conn = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| ToolError::new(format!("CKG store unreadable: {e}")))?;
    let _ = conn.busy_timeout(Duration::from_millis(5000));
    Ok(Some(conn))
}

fn ckg_get_neighborhood(
    conn: &Connection,
    node_id: &str,
    k: i64,
    kind: Option<&str>,
) -> ToolResult<Vec<Value>> {
    // Recursive CTE mirrors Python CkgStore.get_neighborhood.
    let mut stmt = conn
        .prepare(
            r#"
            WITH RECURSIVE nbr(id, depth) AS (
                SELECT ?1, 0
                UNION
                SELECT e.dst, n.depth + 1 FROM ckg_edges e JOIN nbr n ON e.src = n.id
                WHERE n.depth < ?2 AND (?3 IS NULL OR e.kind = ?3)
            )
            SELECT DISTINCT id, depth FROM nbr WHERE id != ?1
            "#,
        )
        .map_err(|e| ToolError::new(format!("CKG prepare failed: {e}")))?;
    let kind_s = kind.map(str::to_string);
    let rows = stmt
        .query_map(params![node_id, k, kind_s], |row| {
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "depth": row.get::<_, i64>(1)?,
            }))
        })
        .map_err(|e| ToolError::new(format!("CKG query failed: {e}")))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| ToolError::new(format!("CKG row: {e}")))?);
    }
    Ok(out)
}

fn ckg_find_imports(conn: &Connection, file: &str) -> ToolResult<Vec<Value>> {
    let mut stmt = conn
        .prepare(
            "SELECT src, dst, kind FROM ckg_edges WHERE src_file = ?1 AND kind = 'IMPORT'",
        )
        .map_err(|e| ToolError::new(format!("CKG prepare failed: {e}")))?;
    let rows = stmt
        .query_map(params![file], |row| {
            Ok(json!({
                "src": row.get::<_, String>(0)?,
                "dst": row.get::<_, String>(1)?,
                "kind": row.get::<_, String>(2)?,
            }))
        })
        .map_err(|e| ToolError::new(format!("CKG query failed: {e}")))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| ToolError::new(format!("CKG row: {e}")))?);
    }
    Ok(out)
}

fn file_id_of_node(node_id: &str) -> &str {
    node_id.split('#').next().unwrap_or(node_id)
}

pub fn get_neighborhood(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    session_token: Option<&str>,
    project_id: &str,
    node_id: &str,
    k: Option<i64>,
    kind: Option<&str>,
) -> ToolResult<Value> {
    let (agent_id, role) = require_agent_tool(
        projects_dir,
        agent_id,
        role,
        "get_neighborhood",
        session_token,
    )?;
    let project_id = normalize_project_id(project_id)?;
    enforce_mini_oracle_project_scope(projects_dir, &agent_id, &role, Some(&project_id))?;
    let node_id = clean_text(node_id, "node_id", 1024)?;
    let k = k.unwrap_or(1).clamp(1, 4);
    let kind = kind.map(str::trim).filter(|s| !s.is_empty());
    let allowed = oracle_allowed_file_ids(projects_dir, Some(&project_id))?;
    if !allowed.contains(file_id_of_node(&node_id)) {
        return Err(ToolError::new("node is not in this project's scope."));
    }
    let db = ckg_db_path(projects_dir);
    let neighborhood = match open_ckg_readonly(&db)? {
        None => vec![],
        Some(conn) => {
            let rows = ckg_get_neighborhood(&conn, &node_id, k, kind)?;
            rows.into_iter()
                .filter(|r| {
                    r.get("id")
                        .and_then(|v| v.as_str())
                        .map(|id| allowed.contains(file_id_of_node(id)))
                        .unwrap_or(false)
                })
                .collect()
        }
    };
    let _ = audit_agent_read(
        projects_dir,
        &agent_id,
        &role,
        "get_neighborhood",
        &format!(
            "Read CKG neighborhood of {node_id} (k={k}, {} nodes).",
            neighborhood.len()
        ),
        Some(&project_id),
    );
    Ok(json!({
        "projectId": project_id,
        "nodeId": node_id,
        "neighborhood": neighborhood,
    }))
}

pub fn find_imports(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    session_token: Option<&str>,
    project_id: &str,
    file: &str,
) -> ToolResult<Value> {
    let (agent_id, role) =
        require_agent_tool(projects_dir, agent_id, role, "find_imports", session_token)?;
    let project_id = normalize_project_id(project_id)?;
    enforce_mini_oracle_project_scope(projects_dir, &agent_id, &role, Some(&project_id))?;
    let file = clean_text(file, "file", 1024)?;
    let allowed = oracle_allowed_file_ids(projects_dir, Some(&project_id))?;
    if !allowed.contains(&file) {
        return Err(ToolError::new("file is not in this project's scope."));
    }
    let db = ckg_db_path(projects_dir);
    let imports = match open_ckg_readonly(&db)? {
        None => vec![],
        Some(conn) => {
            let rows = ckg_find_imports(&conn, &file)?;
            rows.into_iter()
                .filter(|r| {
                    r.get("dst")
                        .and_then(|v| v.as_str())
                        .map(|dst| allowed.contains(file_id_of_node(dst)))
                        .unwrap_or(false)
                })
                .collect()
        }
    };
    let _ = audit_agent_read(
        projects_dir,
        &agent_id,
        &role,
        "find_imports",
        &format!("Read CKG imports of {file} ({} edges).", imports.len()),
        Some(&project_id),
    );
    Ok(json!({
        "projectId": project_id,
        "file": file,
        "imports": imports,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_file::project_path;
    use crate::state::{
        find_session_mut, read_agents_state, seed_launch_pending, with_agents_lock,
        write_agents_state,
    };
    use crate::tools::agent_lifecycle::agent_register;
    use serde_json::json;
    use std::fs;
    use std::sync::{Mutex, OnceLock};
    use tempfile::TempDir;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    fn temp_projects() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let projects = tmp.path().join("projects");
        fs::create_dir_all(&projects).unwrap();
        (tmp, projects)
    }

    fn register(projects: &Path, agent_id: &str, role: &str) -> String {
        let token = format!("launch-{agent_id}");
        seed_launch_pending(projects, agent_id, role, &token).unwrap();
        let ack = agent_register(
            projects,
            agent_id,
            role,
            Some("opus"),
            None,
            Some("reg"),
            Some(&token),
        )
        .unwrap();
        ack["sessionToken"].as_str().unwrap().to_string()
    }

    fn write_proj_with_root(projects: &Path, id: &str, root: &Path) {
        let path = project_path(projects, id).unwrap();
        let content = format!(
            "---\nid: {id}\ntitle: T\nstatus: active\nroot_path: {}\nupdated_at: 2026-01-01T00:00:00Z\n---\n\n```aspis-project\n{{\"version\":1,\"tasks\":[],\"notes\":[]}}\n```\n",
            root.display()
        );
        fs::write(&path, content).unwrap();
    }

    #[test]
    fn structure_fail_closed_without_app_bin() {
        let _g = env_lock();
        for k in APP_BIN_ENVS {
            std::env::remove_var(k);
        }
        let (_tmp, projects) = temp_projects();
        let ws = projects.parent().unwrap().join("ws");
        let root = ws.join("code");
        fs::create_dir_all(&root).unwrap();
        std::env::set_var("ASPIS_WORKSPACE_ROOT", ws.to_str().unwrap());
        write_proj_with_root(&projects, "struct-a", &root);
        let tok = register(&projects, "coder-st", "coder");
        let err = project_structure(
            &projects,
            "coder-st",
            "coder",
            Some(&tok),
            "struct-a",
            false,
        )
        .unwrap_err();
        assert!(
            err.message.contains("unavailable") || err.message.contains("not configured"),
            "{}",
            err.message
        );
        std::env::remove_var("ASPIS_WORKSPACE_ROOT");
    }

    #[test]
    fn neighborhood_out_of_scope_rejected() {
        let _g = env_lock();
        let (_tmp, projects) = temp_projects();
        let ws = projects.parent().unwrap().join("ws2");
        let root = ws.join("code");
        fs::create_dir_all(&root).unwrap();
        std::env::set_var("ASPIS_WORKSPACE_ROOT", ws.to_str().unwrap());
        write_proj_with_root(&projects, "ckg-a", &root);
        let tok = register(&projects, "coder-ckg", "coder");
        // No manifest → empty allowed → any node is out of scope
        let err = get_neighborhood(
            &projects,
            "coder-ckg",
            "coder",
            Some(&tok),
            "ckg-a",
            "secret/file.rs",
            Some(1),
            None,
        )
        .unwrap_err();
        assert!(err.message.contains("not in this project's scope"), "{}", err.message);
        std::env::remove_var("ASPIS_WORKSPACE_ROOT");
    }

    #[test]
    fn find_imports_out_of_scope_rejected() {
        let _g = env_lock();
        let (_tmp, projects) = temp_projects();
        let ws = projects.parent().unwrap().join("ws3");
        let root = ws.join("code");
        fs::create_dir_all(&root).unwrap();
        std::env::set_var("ASPIS_WORKSPACE_ROOT", ws.to_str().unwrap());
        write_proj_with_root(&projects, "ckg-b", &root);
        let tok = register(&projects, "coder-imp", "coder");
        let err = find_imports(
            &projects,
            "coder-imp",
            "coder",
            Some(&tok),
            "ckg-b",
            "other/lib.rs",
        )
        .unwrap_err();
        assert!(err.message.contains("not in this project's scope"), "{}", err.message);
        std::env::remove_var("ASPIS_WORKSPACE_ROOT");
    }

    #[test]
    fn mini_cannot_read_other_project_structure() {
        let _g = env_lock();
        for k in APP_BIN_ENVS {
            std::env::remove_var(k);
        }
        let (_tmp, projects) = temp_projects();
        let ws = projects.parent().unwrap().join("ws4");
        let root = ws.join("code");
        fs::create_dir_all(&root).unwrap();
        std::env::set_var("ASPIS_WORKSPACE_ROOT", ws.to_str().unwrap());
        write_proj_with_root(&projects, "mine", &root);
        write_proj_with_root(&projects, "theirs", &root);
        let tok = register(&projects, "mini-st", "mini");
        with_agents_lock(&projects, || {
            let mut state = read_agents_state(&projects).unwrap();
            if let Some(s) = find_session_mut(&mut state, "mini-st") {
                s.insert("currentProjectId".into(), json!("mine"));
            }
            write_agents_state(&projects, state).unwrap();
            Ok::<(), crate::state::ToolError>(())
        })
        .unwrap();
        let err = project_structure(
            &projects,
            "mini-st",
            "mini",
            Some(&tok),
            "theirs",
            false,
        )
        .unwrap_err();
        assert!(err.message.contains("own project"), "{}", err.message);
        std::env::remove_var("ASPIS_WORKSPACE_ROOT");
    }

    #[test]
    fn k_clamped_and_file_id_split() {
        assert_eq!(file_id_of_node("a/b.rs#1-2-0"), "a/b.rs");
        assert_eq!(file_id_of_node("a/b.rs"), "a/b.rs");
    }
}
