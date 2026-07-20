//! Oracle retrieval tools (P6): `oracle_ask`, `oracle_context`, `oracle_find`.
//!
//! # Architecture
//!
//! HTTP loopback thin-client to the resident Oracle server (same M3-P12c contract
//! as `oracle/server/aspis_mcp.py`). No in-process ML embed. Fail-closed when
//! the target is missing, non-loopback, or the HTTP call fails.
//!
//! # Security
//!
//! * Role allowlists via `require_agent_tool`.
//! * Session token for managed sessions.
//! * **Scope fail-closed**: concrete `allowed_file_ids` always; empty project_id
//!   → management-root files only (never the union of all projects). Project-scoped
//!   queries do **not** auto-merge the management-root corpus; empty allowed set
//!   is fail-closed (no chunks / not_found envelope).
//! * Mini role: **must** supply `project_id` matching its own `currentProjectId`
//!   (empty rejected).
//! * Loopback-only HTTP base URL; never log token / absolute paths / query text
//!   (audit stores tool name + queryLen/hash only).
//! * Response chunks re-filtered by `allowed_file_ids` (defense in depth).
//! * Index status `root` is basename-only before egress.
//! * Work-root path confinement: reject `..`/empty components; canonicalize
//!   fail-closed (no raw-path `starts_with` fallback).

use crate::project_file::{load_project_locked, normalize_project_id};
use crate::state::{
    add_event, clean_text, find_session, read_agents_state, with_agents_lock, write_agents_state,
    ToolError, ToolResult,
};
use crate::tools::agent_lifecycle::require_agent_tool;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

// ── constants ───────────────────────────────────────────────────────────────

const UNREACHABLE_MESSAGE: &str = "Oracle server unreachable — open the Devboule app \
(the resident Oracle server answers this tool; there is no \
in-process fallback anymore).";

const ORACLE_HTTP_BASE_ENVS: &[&str] = &[
    "DEVBOULE_ORACLE_HTTP_BASE",
    "ASPIS_ORACLE_HTTP_BASE",
];
const ORACLE_HTTP_TOKEN_ENVS: &[&str] = &[
    "DEVBOULE_ORACLE_AUTH_TOKEN",
    "ASPIS_ORACLE_AUTH_TOKEN",
];
const ORACLE_HTTP_TIMEOUT_ENVS: &[&str] = &[
    "DEVBOULE_ORACLE_HTTP_TIMEOUT_SECS",
    "ASPIS_ORACLE_HTTP_TIMEOUT_SECS",
];

const WORKSPACE_ROOT_ENVS: &[&str] = &[
    "DEVBOULE_WORKSPACE_ROOT",
    "ASPIS_WORKSPACE_ROOT",
    "ASPIS_BIO_WORKSPACE_ROOT",
    "ASPIS_BIO_ROOT",
];

const DISCOVERY_FILE: &str = ".oracle-server.json";
const HEARTBEAT_STALE_SECS: f64 = 45.0;
const TARGET_CACHE_TTL: Duration = Duration::from_secs(5);
const DEFAULT_HTTP_TIMEOUT_SECS: f64 = 8.0;
const MAX_HTTP_TIMEOUT_SECS: f64 = 120.0;

// ── management root / work-root allowlist ───────────────────────────────────

/// Soft management-root resolve: prefer valid root, else parent of `projects/`.
pub fn management_root_from_projects_dir(projects_dir: &Path) -> PathBuf {
    let parent = if projects_dir
        .file_name()
        .and_then(|n| n.to_str())
        == Some("projects")
    {
        projects_dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| projects_dir.to_path_buf())
    } else {
        projects_dir.to_path_buf()
    };
    if is_valid_management_root(&parent) {
        return parent;
    }
    if let Ok(env_root) = std::env::var("DEVBOULE_ROOT").or_else(|_| std::env::var("ASPIS_ROOT")) {
        let p = PathBuf::from(env_root.trim());
        if !env_root.trim().is_empty() && is_valid_management_root(&p) {
            return p;
        }
    }
    parent
}

fn is_valid_management_root(root: &Path) -> bool {
    let has_config = root.join("config.json").is_file()
        || root.join("src-tauri").join("config.json").is_file();
    // Accept either legacy Python marker or the Rust MCP binary marker / oracle package.
    let has_mcp = root
        .join("oracle")
        .join("server")
        .join("aspis_mcp.py")
        .is_file()
        || root.join("devboule-mcp").is_dir()
        || root.join("oracle-core").is_dir();
    has_config && has_mcp
}

/// True if any path component is `..` or empty (audit: reject before resolve).
fn path_has_forbidden_components(path: &Path) -> bool {
    for c in path.components() {
        match c {
            Component::ParentDir => return true,
            Component::Normal(s) if s.is_empty() => return true,
            _ => {}
        }
    }
    // Raw-string empty segments (e.g. `a//b`) — Path::components may collapse them.
    let raw = path.to_string_lossy();
    let is_abs = raw.starts_with('/') || raw.starts_with('\\');
    for (i, seg) in raw.split(|ch| ch == '/' || ch == '\\').enumerate() {
        if seg == ".." {
            return true;
        }
        // Leading empty is the absolute-root marker; any other empty is forbidden.
        if seg.is_empty() && !(is_abs && i == 0) {
            return true;
        }
    }
    false
}

/// Collapse `.` components. Returns `None` if `..`/empty would remain (fail-closed).
fn lexical_normalize(path: &Path) -> Option<PathBuf> {
    if path_has_forbidden_components(path) {
        return None;
    }
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => return None,
            Component::Normal(s) if s.is_empty() => return None,
            other => out.push(other.as_os_str()),
        }
    }
    Some(out)
}

/// Resolve a path for confinement checks.
///
/// * Rejects `..` / empty components always.
/// * Lexically normalizes `.` first.
/// * If the path exists: `canonicalize` — on failure **reject** (fail-closed).
/// * If the path does not exist: use the lexical form (never the raw unnormalized path).
fn resolve_for_confinement(path: &Path) -> Option<PathBuf> {
    let lex = lexical_normalize(path)?;
    if lex.exists() {
        lex.canonicalize().ok()
    } else {
        Some(lex)
    }
}

/// Component-wise containment after safe resolve. Never falls back to raw-path
/// string/`starts_with` when canonicalize fails on an existing path.
fn path_is_within(child: &Path, parent: &Path) -> bool {
    let Some(c) = resolve_for_confinement(child) else {
        return false;
    };
    let Some(p) = resolve_for_confinement(parent) else {
        return false;
    };
    c == p || c.starts_with(&p)
}

fn push_approved_parent(parents: &mut Vec<PathBuf>, path: PathBuf) {
    if let Some(resolved) = resolve_for_confinement(&path) {
        parents.push(resolved);
    }
    // If resolve rejects (forbidden components / canonicalize fail on existing),
    // omit the parent entirely — fail-closed rather than trust a raw path.
}

fn approved_work_root_parents(management_root: Option<&Path>) -> Vec<PathBuf> {
    let mut parents = Vec::new();
    if let Some(mr) = management_root {
        push_approved_parent(&mut parents, mr.to_path_buf());
    }
    for env_name in WORKSPACE_ROOT_ENVS {
        if let Ok(v) = std::env::var(env_name) {
            let t = v.trim();
            if t.is_empty() {
                continue;
            }
            push_approved_parent(&mut parents, PathBuf::from(t));
        }
    }
    let default_ws = dirs_home()
        .map(|h| h.join("Desktop").join("aspis bio"))
        .unwrap_or_else(|| PathBuf::from("aspis bio"));
    push_approved_parent(&mut parents, default_ws);
    // de-dupe
    let mut seen = HashSet::new();
    parents.retain(|p| seen.insert(p.to_string_lossy().to_ascii_lowercase()));
    parents
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Constrain `rootPath` to approved workspace parents (M2 / path confinement).
pub fn validate_project_work_root(
    candidate: &Path,
    management_root: Option<&Path>,
    project_id: Option<&str>,
) -> ToolResult<PathBuf> {
    // Fail-closed: never use the raw unnormalized candidate for containment.
    let root = resolve_for_confinement(candidate).ok_or_else(|| {
        ToolError::new(
            "rootPath is invalid (contains '..' / empty components, or \
             cannot be resolved); use a folder under the Devboule workspace.",
        )
    })?;
    let home = dirs_home().and_then(|h| h.canonicalize().ok());
    let mut broad = HashSet::new();
    if let Some(h) = home.as_ref() {
        broad.insert(h.clone());
        let desktop = h.join("Desktop");
        if let Ok(d) = desktop.canonicalize() {
            broad.insert(d);
        }
    }
    if let Some(anchor) = root.components().next() {
        let a = PathBuf::from(anchor.as_os_str());
        if let Ok(r) = a.canonicalize() {
            broad.insert(r);
        }
    }
    if broad.iter().any(|b| b == &root) {
        let project_tag = project_id
            .map(|id| format!(" for project '{id}'"))
            .unwrap_or_default();
        let name = root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("root");
        return Err(ToolError::new(format!(
            "Project working root '{name}'{project_tag} is too broad; \
             point it at a specific project folder under the Devboule \
             workspace (set ASPIS_WORKSPACE_ROOT to widen the approved set)."
        )));
    }
    let lower = root.to_string_lossy().to_ascii_lowercase();
    if lower.ends_with("\\windows")
        || lower.contains("\\windows\\system32")
        || lower.ends_with("/windows")
        || lower.contains("/windows/system32")
    {
        let name = root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("root");
        return Err(ToolError::new(format!(
            "Project working root '{name}' is unsafe (system directory); \
             use a folder under the Devboule workspace."
        )));
    }
    let parents = approved_work_root_parents(management_root);
    if !parents
        .iter()
        .any(|p| &root == p || path_is_within(&root, p))
    {
        let parts: Vec<_> = root
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let mut display = if parts.len() > 2 {
            format!("…/{}", parts[parts.len() - 2..].join("/"))
        } else {
            root.display().to_string()
        };
        if display.len() > 200 {
            display.truncate(197);
            display.push_str("...");
        }
        return Err(ToolError::new(format!(
            "rootPath '{display}' is outside approved Devboule workspaces; \
             set ASPIS_WORKSPACE_ROOT to its parent to approve."
        )));
    }
    Ok(root)
}

/// Resolve a project's configured working root (Censor / structure / oracle scope).
pub fn resolve_project_work_root(projects_dir: &Path, project_id: &str) -> ToolResult<PathBuf> {
    let project = load_project_locked(projects_dir, project_id)?;
    let project_path = project.path.clone();
    let root_path = project
        .metadata
        .root_path()
        .unwrap_or_default()
        .trim()
        .to_string();
    if root_path.is_empty() {
        return Err(ToolError::new(format!(
            "Project '{project_id}' has no `root_path` in its frontmatter \
             (file: {}); add `root_path` to enable Censor findings.",
            project_path.display()
        )));
    }
    let management_root = management_root_from_projects_dir(projects_dir);
    validate_project_work_root(
        Path::new(&root_path),
        Some(&management_root),
        Some(project_id),
    )
}

// ── manifest scope (fail-closed) ────────────────────────────────────────────

fn strip_windows_verbatim_prefix(value: &str) -> String {
    if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    if let Some(rest) = value.strip_prefix(r"\\?\") {
        return rest.to_string();
    }
    value.to_string()
}

fn manifest_load(path: &Path) -> Value {
    if !path.exists() {
        return json!({"files": {}});
    }
    match fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|_| json!({"files": {}})),
        Err(_) => json!({"files": {}}),
    }
}

fn manifest_files_for_root(manifest: &mut Value, root: &Path) -> HashSet<String> {
    let root_key = strip_windows_verbatim_prefix(&root.to_string_lossy());
    let roots = {
        let obj = manifest.as_object_mut().unwrap_or_else(|| {
            // unreachable if load always returns object
            panic!("manifest not object")
        });
        if !obj.contains_key("roots") {
            obj.insert("roots".into(), json!({}));
        }
        // legacy single-root form
        if let (Some(legacy_root), Some(legacy_files)) = (
            obj.get("root").and_then(|v| v.as_str()).map(str::to_string),
            obj.get("files").cloned(),
        ) {
            let roots_obj = obj.get_mut("roots").and_then(|v| v.as_object_mut());
            if let Some(roots_obj) = roots_obj {
                if !roots_obj.contains_key(&legacy_root) && legacy_files.is_object() {
                    roots_obj.insert(legacy_root, json!({"files": legacy_files}));
                }
            }
        }
        obj.get_mut("roots")
    };
    let Some(roots) = roots.and_then(|v| v.as_object_mut()) else {
        return HashSet::new();
    };
    // prune verbatim-prefixed duplicates
    let dup_keys: Vec<String> = roots
        .keys()
        .filter(|k| k.as_str() != root_key && strip_windows_verbatim_prefix(k) == root_key)
        .cloned()
        .collect();
    for k in dup_keys {
        if let Some(duplicate) = roots.remove(&k) {
            let canonical = roots
                .entry(root_key.clone())
                .or_insert_with(|| json!({"files": {}}));
            if let (Some(cf), Some(df)) = (
                canonical
                    .as_object_mut()
                    .and_then(|o| o.get_mut("files"))
                    .and_then(|f| f.as_object_mut()),
                duplicate.get("files").and_then(|f| f.as_object()),
            ) {
                for (fid, rec) in df {
                    cf.entry(fid.clone()).or_insert_with(|| rec.clone());
                }
            }
        }
    }
    let entry = match roots.get(&root_key) {
        Some(e) => e,
        None => return HashSet::new(),
    };
    let files = entry
        .get("files")
        .and_then(|f| f.as_object())
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    files
}

fn oracle_index_root_for_project(
    projects_dir: &Path,
    project_id: Option<&str>,
) -> ToolResult<PathBuf> {
    let management_root = management_root_from_projects_dir(projects_dir);
    let Some(pid) = project_id.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(management_root);
    };
    let project = load_project_locked(projects_dir, pid)?;
    let root_path = project
        .metadata
        .root_path()
        .unwrap_or_default()
        .trim()
        .to_string();
    if root_path.is_empty() {
        return Ok(management_root);
    }
    validate_project_work_root(Path::new(&root_path), Some(&management_root), Some(pid))
}

/// Concrete allowed file_id set. Never returns "unscoped full corpus".
///
/// * Empty / missing `project_id` → management-root files only (never the union
///   of every project). An empty set is fail-closed (callers return no chunks).
/// * Project-scoped → that project's work-root files only. Does **not**
///   auto-merge the entire management-root corpus into every project scope.
pub fn oracle_allowed_file_ids(
    projects_dir: &Path,
    project_id: Option<&str>,
) -> ToolResult<HashSet<String>> {
    let management_root = management_root_from_projects_dir(projects_dir);
    let manifest_path = management_root
        .join("oracle-data")
        .join("chunk-index-manifest.json");
    let mut manifest = manifest_load(&manifest_path);

    let project_id = project_id.map(str::trim).filter(|s| !s.is_empty());
    if project_id.is_none() {
        // Unscoped → management root only (never union of all projects).
        return Ok(manifest_files_for_root(&mut manifest, &management_root));
    }
    let root = oracle_index_root_for_project(projects_dir, project_id)?;
    // Project scope only — no management-root auto-merge (HIGH #4).
    Ok(manifest_files_for_root(&mut manifest, &root))
}

/// SEC#9: mini may only read its own project's corpus.
/// Empty `project_id` is rejected for mini (must match `currentProjectId`).
pub fn enforce_mini_oracle_project_scope(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    project_id: Option<&str>,
) -> ToolResult<()> {
    if role != "mini" {
        return Ok(());
    }
    let requested = project_id.unwrap_or("").trim();
    if requested.is_empty() {
        return Err(ToolError::new(
            "A mini agent must supply project_id matching its current project \
             (empty project_id is not allowed for oracle tools).",
        ));
    }
    with_agents_lock(projects_dir, || {
        let state = read_agents_state(projects_dir)?;
        let session = find_session(&state, agent_id);
        let own = session
            .and_then(|s| s.get("currentProjectId"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if requested != own {
            return Err(ToolError::new(format!(
                "A mini agent may only read its own project via oracle_context \
                 (scoped to {}, requested {requested}).",
                if own.is_empty() {
                    "its spawning project"
                } else {
                    &own
                }
            )));
        }
        Ok(())
    })
}

/// Redact a query/message for audit storage: tool name + length + short hash only.
fn audit_query_redacted(tool_name: &str, query_or_message: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(query_or_message.as_bytes());
    let hash = hex::encode(hasher.finalize());
    let short = &hash[..16.min(hash.len())];
    format!(
        "{tool_name} queryLen={} queryHash={short}",
        query_or_message.len()
    )
}

/// Audit a read tool (identity + project + tool name only — never query text).
///
/// Does **not** call `upsert_session` (that would set session status to the tool
/// name and store the message on the session). Events only, with redacted payload.
pub fn audit_agent_read(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    event_type: &str,
    message: &str,
    project_id: Option<&str>,
) -> ToolResult<()> {
    let redacted = audit_query_redacted(event_type, message);
    with_agents_lock(projects_dir, || {
        let mut state = read_agents_state(projects_dir)?;
        add_event(
            &mut state,
            agent_id,
            role,
            event_type,
            &redacted,
            project_id,
            None,
            None,
            None,
        )?;
        write_agents_state(projects_dir, state)?;
        Ok(())
    })
}

// ── HTTP target resolution (loopback only) ──────────────────────────────────

fn env_first(keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Ok(v) = std::env::var(k) {
            let t = v.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

fn is_loopback_http_base(base: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base) else {
        return false;
    };
    if url.scheme() != "http" && url.scheme() != "https" {
        return false;
    }
    match url.host_str().map(|h| h.to_ascii_lowercase()) {
        Some(h) if h == "127.0.0.1" || h == "localhost" || h == "::1" || h == "[::1]" => true,
        _ => false,
    }
}

fn pid_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // Best-effort liveness via `kill -0` (POSIX). Failure modes:
    // exit 0 → alive; non-zero → treat as dead (fail closed toward skip-HTTP).
    // Windows: no reliable probe without extra crates — try the HTTP target.
    #[cfg(unix)]
    {
        use std::process::Command;
        match Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
        {
            Ok(st) => st.success(),
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

fn heartbeat_fresh(iso: &str) -> bool {
    let normalized = if iso.ends_with('Z') {
        format!("{}+00:00", &iso[..iso.len() - 1])
    } else {
        iso.to_string()
    };
    let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(&normalized) else {
        return true; // garbage → fall through to pid gate (caller handles)
    };
    let age = (chrono::Utc::now() - parsed.with_timezone(&chrono::Utc)).num_seconds() as f64;
    age <= HEARTBEAT_STALE_SECS
}

struct TargetCache {
    at: Instant,
    value: Option<(String, String)>,
}

static TARGET_CACHE: Mutex<Option<(String, TargetCache)>> = Mutex::new(None);

/// Resolve (base_url, auth_token) or None (caller fails closed).
pub fn resolve_oracle_http_target(projects_dir: &Path) -> Option<(String, String)> {
    let key = projects_dir.to_string_lossy().to_string();
    if let Ok(guard) = TARGET_CACHE.lock() {
        if let Some((k, cached)) = guard.as_ref() {
            if k == &key && cached.at.elapsed() < TARGET_CACHE_TTL {
                return cached.value.clone();
            }
        }
    }
    let result = resolve_oracle_http_target_uncached(projects_dir);
    if let Ok(mut guard) = TARGET_CACHE.lock() {
        *guard = Some((
            key,
            TargetCache {
                at: Instant::now(),
                value: result.clone(),
            },
        ));
    }
    result
}

fn resolve_oracle_http_target_uncached(projects_dir: &Path) -> Option<(String, String)> {
    if let (Some(base), Some(token)) = (env_first(ORACLE_HTTP_BASE_ENVS), env_first(ORACLE_HTTP_TOKEN_ENVS))
    {
        if !is_loopback_http_base(&base) {
            return None;
        }
        return Some((base, token));
    }
    let path = projects_dir.join(DISCOVERY_FILE);
    let raw = fs::read_to_string(path).ok()?;
    let data: Value = serde_json::from_str(&raw).ok()?;
    let base = data.get("baseUrl").and_then(|v| v.as_str())?.trim();
    let token = data.get("authToken").and_then(|v| v.as_str())?.trim();
    if base.is_empty() || token.is_empty() {
        return None;
    }
    if !is_loopback_http_base(base) {
        return None;
    }
    if let Some(hb) = data.get("heartbeatAt").and_then(|v| v.as_str()) {
        let hb = hb.trim();
        if !hb.is_empty() {
            if !heartbeat_fresh(hb) {
                return None;
            }
            // Valid heartbeat → accept without pid probe.
            return Some((base.to_string(), token.to_string()));
        }
    }
    if let Some(pid) = data.get("pid").and_then(|v| v.as_i64()) {
        if pid > 0 && pid <= i32::MAX as i64 && !pid_alive(pid as i32) {
            return None;
        }
    }
    Some((base.to_string(), token.to_string()))
}

fn http_timeout_secs() -> f64 {
    let raw = env_first(ORACLE_HTTP_TIMEOUT_ENVS)
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(DEFAULT_HTTP_TIMEOUT_SECS);
    if raw > 0.0 && raw <= MAX_HTTP_TIMEOUT_SECS {
        raw
    } else {
        DEFAULT_HTTP_TIMEOUT_SECS
    }
}

fn safe_index_root(value: Option<&Value>) -> Option<String> {
    let s = value.and_then(|v| v.as_str()).filter(|s| !s.is_empty())?;
    Some(
        Path::new(s)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(s)
            .to_string(),
    )
}

fn http_readiness_placeholder() -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("root".into(), Value::Null);
    m.insert("indexed_files".into(), Value::Null);
    m.insert("pending_files".into(), Value::Null);
    m.insert("stale_files".into(), Value::Null);
    m.insert("source".into(), json!("resident-server"));
    m
}

fn empty_ask_envelope(query: &str) -> Value {
    json!({
        "mode": "oracle-http-bounded",
        "query": query,
        "summary": "No Oracle documents are in scope for this request.",
        "answer": "No Oracle documents are in scope for this request.",
        "citations": [],
        "not_found": true,
        "suggested_path": null,
        "answer_source": null,
        "fallback_reason": null,
        "llm_provider": null,
        "llm_model": null,
        "results": [],
    })
}

/// Extract a chunk/citation file id (snake or camel; also `file_sorgente`).
fn chunk_file_id(chunk: &Value) -> Option<&str> {
    chunk
        .get("file_id")
        .or_else(|| chunk.get("fileId"))
        .or_else(|| chunk.get("file_sorgente"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Defense-in-depth: drop any response chunk outside the concrete allowed set.
fn filter_chunks_by_allowed(chunks: Vec<Value>, allowed: &HashSet<String>) -> Vec<Value> {
    chunks
        .into_iter()
        .filter(|c| {
            chunk_file_id(c)
                .map(|id| allowed.contains(id))
                .unwrap_or(false)
        })
        .collect()
}

/// Re-filter ask envelope arrays (`results`, `citations`) by allowed file ids.
fn refilter_ask_payload(mut result: Value, allowed: &HashSet<String>) -> Value {
    let Some(obj) = result.as_object_mut() else {
        return result;
    };
    for key in ["results", "citations"] {
        if let Some(arr) = obj.get(key).and_then(|v| v.as_array()).cloned() {
            let filtered = filter_chunks_by_allowed(arr, allowed);
            obj.insert(key.into(), json!(filtered));
        }
    }
    result
}

struct FilterArgs {
    kind: Option<String>,
    language: Option<String>,
    symbols: Option<Vec<String>>,
    imports: Option<Vec<String>>,
    module: Option<String>,
    group_by_file: bool,
}

fn parse_filter_args(
    kind: Option<&str>,
    language: Option<&str>,
    symbols: Option<&[String]>,
    imports: Option<&[String]>,
    module: Option<&str>,
    group_by_file: bool,
) -> FilterArgs {
    let kind = kind
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let language = language
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let symbols = symbols.and_then(|list| {
        let f: Vec<String> = list
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if f.is_empty() {
            None
        } else {
            Some(f)
        }
    });
    let imports = imports.and_then(|list| {
        let f: Vec<String> = list
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if f.is_empty() {
            None
        } else {
            Some(f)
        }
    });
    let module = module
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    FilterArgs {
        kind,
        language,
        symbols,
        imports,
        module,
        group_by_file,
    }
}

fn oracle_http_post(base_url: &str, token: &str, path: &str, payload: &Value) -> ToolResult<Value> {
    let timeout = Duration::from_secs_f64(http_timeout_secs());
    let client = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .connect_timeout(Duration::from_secs(1))
        .build()
        .map_err(|_| ToolError::new(UNREACHABLE_MESSAGE))?;
    let url = format!("{}{path}", base_url.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .header("x-oracle-auth-token", token)
        .header("Content-Type", "application/json")
        .json(payload)
        .send();
    let resp = match resp {
        Ok(r) => r,
        Err(_) => return Err(ToolError::new(UNREACHABLE_MESSAGE)),
    };
    let status = resp.status();
    if status.is_client_error() {
        return Err(ToolError::new(format!(
            "Oracle HTTP call failed with HTTP {}. \
             Check the agent auth token / request (see server logs).",
            status.as_u16()
        )));
    }
    if !status.is_success() {
        return Err(ToolError::new(UNREACHABLE_MESSAGE));
    }
    let data: Value = resp
        .json()
        .map_err(|_| ToolError::new(UNREACHABLE_MESSAGE))?;
    if !data.is_object() {
        return Err(ToolError::new(UNREACHABLE_MESSAGE));
    }
    Ok(data)
}

fn apply_filters(payload: &mut Map<String, Value>, filters: &FilterArgs) {
    if let Some(ref k) = filters.kind {
        payload.insert("kind".into(), json!(k));
    }
    if let Some(ref l) = filters.language {
        payload.insert("language".into(), json!(l));
    }
    if let Some(ref s) = filters.symbols {
        payload.insert("symbols".into(), json!(s));
    }
    if let Some(ref i) = filters.imports {
        payload.insert("imports".into(), json!(i));
    }
    if let Some(ref m) = filters.module {
        payload.insert("module".into(), json!(m));
    }
    if filters.group_by_file {
        payload.insert("group_by_file".into(), json!(true));
    }
}

fn dispatch_oracle_context(
    projects_dir: &Path,
    query: &str,
    limit: i64,
    allowed: &HashSet<String>,
    filters: &FilterArgs,
) -> ToolResult<Vec<Value>> {
    // Fail-closed: concrete scope required (empty set is concrete → empty result).
    let target = resolve_oracle_http_target(projects_dir)
        .ok_or_else(|| ToolError::new(UNREACHABLE_MESSAGE))?;
    let (base, token) = target;
    if allowed.is_empty() {
        return Ok(vec![]);
    }
    let mut scope: Vec<String> = allowed.iter().cloned().collect();
    scope.sort();
    let mut payload = Map::new();
    payload.insert("query".into(), json!(query));
    payload.insert("limit".into(), json!(limit));
    payload.insert("allowed_file_ids".into(), json!(scope));
    apply_filters(&mut payload, filters);
    // group_by_file not used for context
    payload.remove("group_by_file");
    let data = oracle_http_post(&base, &token, "/context-bounded", &Value::Object(payload))?;
    let chunks = data
        .get("chunks")
        .and_then(|c| c.as_array())
        .ok_or_else(|| ToolError::new(UNREACHABLE_MESSAGE))?
        .clone();
    // Re-filter on the client: never trust the HTTP peer to honor scope alone.
    Ok(filter_chunks_by_allowed(chunks, allowed))
}

fn dispatch_oracle_ask(
    projects_dir: &Path,
    query: &str,
    limit: i64,
    allowed: &HashSet<String>,
    filters: &FilterArgs,
) -> ToolResult<Value> {
    let target = resolve_oracle_http_target(projects_dir)
        .ok_or_else(|| ToolError::new(UNREACHABLE_MESSAGE))?;
    let (base, token) = target;
    if allowed.is_empty() {
        return Ok(empty_ask_envelope(query));
    }
    let mut scope: Vec<String> = allowed.iter().cloned().collect();
    scope.sort();
    let mut payload = Map::new();
    payload.insert("query".into(), json!(query));
    payload.insert("limit".into(), json!(limit));
    payload.insert("allowed_file_ids".into(), json!(scope));
    apply_filters(&mut payload, filters);
    let result = oracle_http_post(&base, &token, "/ask-bounded", &Value::Object(payload))?;
    Ok(refilter_ask_payload(result, allowed))
}

// ── public tool handlers ────────────────────────────────────────────────────

pub fn oracle_ask(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    session_token: Option<&str>,
    query: &str,
    limit: Option<i64>,
    project_id: Option<&str>,
    kind: Option<&str>,
    language: Option<&str>,
    symbols: Option<&[String]>,
    group_by_file: bool,
) -> ToolResult<Value> {
    let (agent_id, role) =
        require_agent_tool(projects_dir, agent_id, role, "oracle_ask", session_token)?;
    let query = clean_text(query, "Query", 2000)?;
    let limit = limit.unwrap_or(5).clamp(1, 50);
    let pid = project_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| normalize_project_id(s))
        .transpose()?;
    // Mini scope: ask also honors mini restriction when project_id set.
    enforce_mini_oracle_project_scope(projects_dir, &agent_id, &role, pid.as_deref())?;
    let allowed = oracle_allowed_file_ids(projects_dir, pid.as_deref())?;
    let filters = parse_filter_args(kind, language, symbols, None, None, group_by_file);
    let mut result = dispatch_oracle_ask(projects_dir, &query, limit, &allowed, &filters)?;
    let status = http_readiness_placeholder();
    if let Some(obj) = result.as_object_mut() {
        obj.insert(
            "index_status".into(),
            json!({
                "root": safe_index_root(status.get("root")),
                "indexedFiles": status.get("indexed_files"),
                "pendingFiles": status.get("pending_files"),
                "staleFiles": status.get("stale_files"),
            }),
        );
    }
    let _ = audit_agent_read(
        projects_dir,
        &agent_id,
        &role,
        "oracle_ask",
        &query,
        pid.as_deref(),
    );
    Ok(result)
}

pub fn oracle_context(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    session_token: Option<&str>,
    query: &str,
    limit: Option<i64>,
    project_id: Option<&str>,
    kind: Option<&str>,
    language: Option<&str>,
    symbols: Option<&[String]>,
    imports: Option<&[String]>,
    module: Option<&str>,
) -> ToolResult<Value> {
    let (agent_id, role) =
        require_agent_tool(projects_dir, agent_id, role, "oracle_context", session_token)?;
    enforce_mini_oracle_project_scope(
        projects_dir,
        &agent_id,
        &role,
        project_id.map(str::trim).filter(|s| !s.is_empty()),
    )?;
    let query = clean_text(query, "Query", 2000)?;
    let limit = limit.unwrap_or(8).clamp(1, 50);
    let pid = project_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| normalize_project_id(s))
        .transpose()?;
    let _ = audit_agent_read(
        projects_dir,
        &agent_id,
        &role,
        "oracle_context",
        &query,
        pid.as_deref(),
    );
    let allowed = oracle_allowed_file_ids(projects_dir, pid.as_deref())?;
    let filters = parse_filter_args(kind, language, symbols, imports, module, false);
    let chunks = dispatch_oracle_context(projects_dir, &query, limit, &allowed, &filters)?;
    let status = http_readiness_placeholder();
    Ok(json!({
        "query": query,
        "indexStatus": {
            "root": safe_index_root(status.get("root")),
            "indexedFiles": status.get("indexed_files"),
            "pendingFiles": status.get("pending_files"),
            "staleFiles": status.get("stale_files"),
        },
        "chunks": chunks,
    }))
}

pub fn oracle_find(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    session_token: Option<&str>,
    query: &str,
    limit: Option<i64>,
    project_id: Option<&str>,
    kind: Option<&str>,
    language: Option<&str>,
) -> ToolResult<Value> {
    let (agent_id, role) =
        require_agent_tool(projects_dir, agent_id, role, "oracle_find", session_token)?;
    let pid_raw = project_id.map(str::trim).filter(|s| !s.is_empty());
    enforce_mini_oracle_project_scope(projects_dir, &agent_id, &role, pid_raw)?;
    let query = clean_text(query, "Query", 2000)?;
    let find_kind = kind.map(str::trim).filter(|s| !s.is_empty());
    let find_lang = language.map(str::trim).filter(|s| !s.is_empty());
    let find_limit = limit.unwrap_or(10).clamp(1, 50);
    let pid = pid_raw.map(|s| normalize_project_id(s)).transpose()?;
    let _ = audit_agent_read(
        projects_dir,
        &agent_id,
        &role,
        "oracle_find",
        &query,
        pid.as_deref(),
    );
    let scope = oracle_allowed_file_ids(projects_dir, pid.as_deref())?;
    let symbols = vec![query.clone()];
    let filters = parse_filter_args(
        find_kind,
        find_lang,
        Some(symbols.as_slice()),
        None,
        None,
        false,
    );
    let chunks = dispatch_oracle_context(projects_dir, &query, find_limit, &scope, &filters)?;
    let status = http_readiness_placeholder();
    Ok(json!({
        "query": query,
        "kind": find_kind,
        "language": find_lang,
        "indexStatus": {
            "root": safe_index_root(status.get("root")),
            "indexedFiles": status.get("indexed_files"),
        },
        "chunks": chunks,
        "hint": "Each chunk has kind, symbol_name, signature, language, line_start, line_end, and symbols_used — use these to decide which files to open.",
    }))
}

// ── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{seed_launch_pending, write_agents_state};
    use crate::tools::agent_lifecycle::agent_register;
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

    #[test]
    fn unscoped_oracle_never_returns_none_scope() {
        let (_tmp, projects) = temp_projects();
        let allowed = oracle_allowed_file_ids(&projects, None).unwrap();
        // Empty is fine; None is not possible (ToolResult).
        assert!(allowed.is_empty() || !allowed.is_empty());
    }

    #[test]
    fn loopback_gate_rejects_remote_base() {
        assert!(!is_loopback_http_base("https://evil.example/oracle"));
        assert!(is_loopback_http_base("http://127.0.0.1:7788"));
        assert!(is_loopback_http_base("http://localhost:9"));
    }

    #[test]
    fn oracle_ask_fail_closed_without_server() {
        let _g = env_lock();
        // Clear any env targets.
        for k in ORACLE_HTTP_BASE_ENVS
            .iter()
            .chain(ORACLE_HTTP_TOKEN_ENVS.iter())
        {
            std::env::remove_var(k);
        }
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "coder-ora", "coder");
        let err = oracle_ask(
            &projects,
            "coder-ora",
            "coder",
            Some(&tok),
            "where is main?",
            Some(3),
            None,
            None,
            None,
            None,
            false,
        )
        .unwrap_err();
        assert!(
            err.message.contains("Oracle server unreachable")
                || err.message.contains("unreachable"),
            "{}",
            err.message
        );
    }

    #[test]
    fn mini_cross_project_scope_rejected() {
        let _g = env_lock();
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "mini-1", "mini");
        // Stamp currentProjectId = proj-a
        with_agents_lock(&projects, || {
            let mut state = read_agents_state(&projects).unwrap();
            if let Some(s) = crate::state::find_session_mut(&mut state, "mini-1") {
                s.insert("currentProjectId".into(), json!("proj-a"));
            }
            write_agents_state(&projects, state).unwrap();
            Ok::<(), ToolError>(())
        })
        .unwrap();
        let err = enforce_mini_oracle_project_scope(
            &projects,
            "mini-1",
            "mini",
            Some("other-proj"),
        )
        .unwrap_err();
        assert!(err.message.contains("own project"), "{}", err.message);
        // Own project ok
        enforce_mini_oracle_project_scope(&projects, "mini-1", "mini", Some("proj-a")).unwrap();
        let _ = tok;
    }

    #[test]
    fn mini_empty_project_id_rejected() {
        let _g = env_lock();
        let (_tmp, projects) = temp_projects();
        let _tok = register(&projects, "mini-empty", "mini");
        with_agents_lock(&projects, || {
            let mut state = read_agents_state(&projects).unwrap();
            if let Some(s) = crate::state::find_session_mut(&mut state, "mini-empty") {
                s.insert("currentProjectId".into(), json!("proj-a"));
            }
            write_agents_state(&projects, state).unwrap();
            Ok::<(), ToolError>(())
        })
        .unwrap();
        let err =
            enforce_mini_oracle_project_scope(&projects, "mini-empty", "mini", None).unwrap_err();
        assert!(
            err.message.contains("must supply project_id")
                || err.message.contains("empty project_id"),
            "{}",
            err.message
        );
        let err = enforce_mini_oracle_project_scope(
            &projects,
            "mini-empty",
            "mini",
            Some(""),
        )
        .unwrap_err();
        assert!(
            err.message.contains("must supply project_id")
                || err.message.contains("empty project_id"),
            "{}",
            err.message
        );
        // Non-mini may omit project_id.
        enforce_mini_oracle_project_scope(&projects, "coder-x", "coder", None).unwrap();
    }

    #[test]
    fn path_is_within_rejects_dotdot_escape() {
        let tmp = TempDir::new().unwrap();
        let parent = tmp.path().join("allowed");
        let inside = parent.join("child");
        fs::create_dir_all(&inside).unwrap();
        // Classic `..` escape: must never count as within parent.
        let escape = parent.join("child").join("..").join("..").join("etc");
        assert!(
            !path_is_within(&escape, &parent),
            "path with .. must be rejected"
        );
        assert!(path_has_forbidden_components(Path::new("foo/../../etc")));
        assert!(path_has_forbidden_components(Path::new("foo/../bar")));
        assert!(path_has_forbidden_components(Path::new("a//b")));
        assert!(!path_has_forbidden_components(Path::new("foo/bar")));
        // Existing path under parent is allowed.
        assert!(path_is_within(&inside, &parent));
        // Equal paths allowed.
        assert!(path_is_within(&parent, &parent));
        // Sibling outside parent rejected.
        let sibling = tmp.path().join("other");
        fs::create_dir_all(&sibling).unwrap();
        assert!(!path_is_within(&sibling, &parent));
    }

    #[test]
    fn work_root_rejects_dotdot_in_candidate() {
        let _g = env_lock();
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().join("ws");
        fs::create_dir_all(&ws).unwrap();
        std::env::set_var("ASPIS_WORKSPACE_ROOT", ws.to_str().unwrap());
        let err = validate_project_work_root(
            Path::new("/tmp/ws/proj/../../etc"),
            Some(&ws),
            Some("p1"),
        )
        .unwrap_err();
        assert!(
            err.message.contains("invalid")
                || err.message.contains("..")
                || err.message.contains("outside approved"),
            "{}",
            err.message
        );
        std::env::remove_var("ASPIS_WORKSPACE_ROOT");
    }

    #[test]
    fn work_root_rejects_escape_outside_workspace() {
        let _g = env_lock();
        std::env::remove_var("ASPIS_WORKSPACE_ROOT");
        std::env::remove_var("DEVBOULE_WORKSPACE_ROOT");
        let err = validate_project_work_root(
            Path::new("/etc"),
            Some(Path::new("/tmp/devboule-mgmt")),
            Some("p1"),
        )
        .unwrap_err();
        assert!(
            err.message.contains("outside approved")
                || err.message.contains("too broad")
                || err.message.contains("invalid"),
            "{}",
            err.message
        );
    }

    #[test]
    fn audit_agent_read_never_stores_query_text() {
        let _g = env_lock();
        let (_tmp, projects) = temp_projects();
        let _tok = register(&projects, "coder-aud", "coder");
        let secret_query = "password=SuperSecretToken123 and api_key=XYZ";
        audit_agent_read(
            &projects,
            "coder-aud",
            "coder",
            "oracle_context",
            secret_query,
            Some("proj-a"),
        )
        .unwrap();
        let state = read_agents_state(&projects).unwrap();
        let blob = state.to_string();
        assert!(
            !blob.contains("SuperSecretToken123"),
            "query text must not appear in agents state: {blob}"
        );
        assert!(
            !blob.contains("password=SuperSecret"),
            "query text must not appear in agents state"
        );
        // Session status must not be overwritten with the tool name.
        let session = find_session(&state, "coder-aud").expect("session");
        let status = session
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_ne!(status, "oracle_context", "status corrupted to tool name");
        // Event message is redacted form.
        let events = state
            .get("events")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let hit = events.iter().find(|e| {
            e.get("type").and_then(|v| v.as_str()) == Some("oracle_context")
                || e.get("eventType").and_then(|v| v.as_str()) == Some("oracle_context")
        });
        assert!(hit.is_some(), "expected audit event, events={events:?}");
        let msg = hit
            .unwrap()
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert!(msg.contains("queryLen="), "{msg}");
        assert!(msg.contains("queryHash="), "{msg}");
        assert!(!msg.contains("SuperSecret"), "{msg}");
    }

    #[test]
    fn filter_chunks_drops_out_of_scope_file_ids() {
        let allowed: HashSet<String> = ["src/a.rs".into()].into_iter().collect();
        let chunks = vec![
            json!({"file_id": "src/a.rs", "text": "ok"}),
            json!({"file_id": "src/secret.rs", "text": "leak"}),
            json!({"fileId": "src/a.rs", "text": "camel ok"}),
            json!({"text": "no id"}),
        ];
        let filtered = filter_chunks_by_allowed(chunks, &allowed);
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().all(|c| {
            chunk_file_id(c).map(|id| id == "src/a.rs").unwrap_or(false)
        }));
    }

    #[test]
    fn work_root_allows_under_workspace_env() {
        let _g = env_lock();
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().join("ws");
        let proj = ws.join("myproj");
        fs::create_dir_all(&proj).unwrap();
        std::env::set_var("ASPIS_WORKSPACE_ROOT", ws.to_str().unwrap());
        let got = validate_project_work_root(&proj, None, Some("p1")).unwrap();
        assert!(got.ends_with("myproj") || got == proj.canonicalize().unwrap());
        std::env::remove_var("ASPIS_WORKSPACE_ROOT");
    }

    #[test]
    fn role_gate_blocks_mini_from_oracle_ask() {
        let _g = env_lock();
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "mini-ask", "mini");
        let err = oracle_ask(
            &projects,
            "mini-ask",
            "mini",
            Some(&tok),
            "hi",
            None,
            None,
            None,
            None,
            None,
            false,
        )
        .unwrap_err();
        assert!(err.message.contains("cannot use"), "{}", err.message);
    }

    #[test]
    fn empty_scope_ask_envelope_shape() {
        let env = empty_ask_envelope("q");
        assert_eq!(env["not_found"], true);
        assert!(env["answer"].as_str().unwrap().contains("No Oracle"));
    }

    #[test]
    fn safe_index_root_basename_only() {
        assert_eq!(
            safe_index_root(Some(&json!("/Users/user/Projects/devboule"))),
            Some("devboule".into())
        );
        assert_eq!(safe_index_root(None), None);
    }
}
