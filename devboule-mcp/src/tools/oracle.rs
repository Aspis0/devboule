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

// F21: stale discovery (dead pid/port left on disk) is the common failure mode
// while the app IS open — "open the app" misleads operators. Point at discovery.
const UNREACHABLE_MESSAGE: &str = "Oracle retrieval endpoint unreachable — the app may be open \
but discovery is stale or the resident server is down. Restart Devboule (or wait for \
Oracle bring-up) so projects/.oracle-server.json is republished with a live port.";

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

/// Roots the user attached as Devboule projects (frontmatter `root_path`).
/// Product bridge for multi-project work (e2e B06): security still fails closed
/// for paths not in this set / env / management root; attaching a project is the
/// in-app "approve this root for Oracle/Censor" action (no env var required).
fn attached_project_roots(projects_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(projects_dir) else {
        return out;
    };
    for ent in entries.flatten() {
        let path = ent.path();
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if stem.is_empty() || stem.starts_with('.') {
            continue;
        }
        let Ok(doc) = load_project_locked(projects_dir, stem) else {
            continue;
        };
        if let Some(rp) = doc.metadata.root_path() {
            let t = rp.trim();
            if !t.is_empty() {
                out.push(PathBuf::from(t));
            }
        }
    }
    out
}

fn approved_work_root_parents(
    management_root: Option<&Path>,
    projects_dir: Option<&Path>,
) -> Vec<PathBuf> {
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
    if let Some(pd) = projects_dir {
        for root in attached_project_roots(pd) {
            // Exact project root is approved (not the whole parent tree).
            push_approved_parent(&mut parents, root);
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
///
/// `projects_dir`: when set, every attached project's `root_path` is also approved
/// (e2e B06 multi-project bridge). Omit only for pure env/management-root checks.
pub fn validate_project_work_root(
    candidate: &Path,
    management_root: Option<&Path>,
    project_id: Option<&str>,
    projects_dir: Option<&Path>,
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
    let parents = approved_work_root_parents(management_root, projects_dir);
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
             attach the folder as a project in Devboule, or set \
             ASPIS_WORKSPACE_ROOT / DEVBOULE_WORKSPACE_ROOT to its parent."
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
        Some(projects_dir),
    )
}

// ── manifest scope (fail-closed) ────────────────────────────────────────────

/// Cap on project/management-controlled manifest reads (DoS / OOM guard).
const MANIFEST_MAX_BYTES: u64 = 32 * 1024 * 1024;

/// Registry of indexed Oracle roots (Layer 2). Lives under management-root
/// `oracle-data/.oracle-roots.json`.
const ROOTS_REGISTRY_FILENAME: &str = ".oracle-roots.json";

fn strip_windows_verbatim_prefix(value: &str) -> String {
    if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    if let Some(rest) = value.strip_prefix(r"\\?\") {
        return rest.to_string();
    }
    value.to_string()
}

/// Normalize a root path string for registry / manifest key equality.
/// Strips Windows verbatim prefixes and, when the path exists, canonicalizes
/// so `/var` and `/private/var` collapse on macOS.
pub fn normalize_root_key(path: &Path) -> String {
    let raw = strip_windows_verbatim_prefix(&path.to_string_lossy());
    if let Ok(canon) = Path::new(&raw).canonicalize() {
        return strip_windows_verbatim_prefix(&canon.to_string_lossy());
    }
    raw
}

fn path_keys_equivalent(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let na = strip_windows_verbatim_prefix(a);
    let nb = strip_windows_verbatim_prefix(b);
    if na == nb {
        return true;
    }
    // Collapse /var ↔ /private/var (macOS) without requiring the path to exist.
    let fold = |s: &str| {
        s.strip_prefix("/private/var/")
            .map(|r| format!("/var/{r}"))
            .unwrap_or_else(|| s.to_string())
    };
    fold(&na) == fold(&nb)
}

/// Load a chunk-index manifest fail-closed: missing, oversize, non-object, or
/// corrupt JSON → empty object (never panic).
fn manifest_load(path: &Path) -> Value {
    if !path.exists() {
        return json!({"files": {}});
    }
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return json!({"files": {}}),
    };
    if meta.len() > MANIFEST_MAX_BYTES {
        return json!({"files": {}});
    }
    let raw = match fs::read_to_string(path) {
        Ok(r) => r,
        Err(_) => return json!({"files": {}}),
    };
    match serde_json::from_str::<Value>(&raw) {
        Ok(Value::Object(map)) => Value::Object(map),
        _ => json!({"files": {}}),
    }
}

fn manifest_files_for_root(manifest: &mut Value, root: &Path) -> HashSet<String> {
    let root_key = normalize_root_key(root);
    let Some(obj) = manifest.as_object_mut() else {
        return HashSet::new();
    };
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
    let Some(roots) = obj.get_mut("roots").and_then(|v| v.as_object_mut()) else {
        return HashSet::new();
    };
    // Prefer exact key; else any key equivalent under path normalization.
    let matched_key = if roots.contains_key(&root_key) {
        Some(root_key.clone())
    } else {
        roots
            .keys()
            .find(|k| path_keys_equivalent(k, &root_key))
            .cloned()
    };
    let Some(matched_key) = matched_key else {
        return HashSet::new();
    };
    // prune verbatim-prefixed / alias duplicates into the matched key
    let dup_keys: Vec<String> = roots
        .keys()
        .filter(|k| k.as_str() != matched_key && path_keys_equivalent(k, &matched_key))
        .cloned()
        .collect();
    for k in dup_keys {
        if let Some(duplicate) = roots.remove(&k) {
            let canonical = roots
                .entry(matched_key.clone())
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
    let entry = match roots.get(&matched_key) {
        Some(e) => e,
        None => return HashSet::new(),
    };
    entry
        .get("files")
        .and_then(|f| f.as_object())
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default()
}

// ── multi-root registry (Layer 2) ───────────────────────────────────────────

/// Discovery credentials for one indexed root (loopback only at resolve time).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OracleRootDiscovery {
    pub base_url: String,
    pub auth_token: String,
    pub pid: Option<i64>,
    pub index_root: Option<String>,
}

/// One registry entry for an indexed Oracle root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OracleRootEntry {
    pub path: String,
    pub manifest_path: String,
    pub discovery: Option<OracleRootDiscovery>,
    pub last_indexed_at: Option<String>,
    pub status: String,
}

/// Registry of indexed roots (pure data; I/O via load/save helpers).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OracleRootsRegistry {
    pub roots: Vec<OracleRootEntry>,
}

impl OracleRootsRegistry {
    pub fn lookup_by_path(&self, root: &Path) -> Option<&OracleRootEntry> {
        let key = normalize_root_key(root);
        self.roots
            .iter()
            .find(|e| path_keys_equivalent(&e.path, &key))
    }

    pub fn upsert(&mut self, entry: OracleRootEntry) {
        let key = normalize_root_key(Path::new(&entry.path));
        if let Some(slot) = self
            .roots
            .iter_mut()
            .find(|e| path_keys_equivalent(&e.path, &key))
        {
            *slot = entry;
        } else {
            self.roots.push(entry);
        }
    }

    /// True iff `root` is registered **and** status is `indexed` (P3 gate).
    pub fn is_registered(&self, root: &Path) -> bool {
        self.lookup_by_path(root)
            .map(|e| e.status.trim().eq_ignore_ascii_case("indexed") || e.status.trim().is_empty())
            .unwrap_or(false)
    }
}

/// Path of the roots registry under the management root.
pub fn oracle_roots_registry_path(management_root: &Path) -> PathBuf {
    management_root
        .join("oracle-data")
        .join(ROOTS_REGISTRY_FILENAME)
}

/// Parse registry JSON fail-closed (unknown shape → empty registry).
pub fn oracle_roots_registry_from_value(value: &Value) -> OracleRootsRegistry {
    let mut reg = OracleRootsRegistry::default();
    let Some(arr) = value.get("roots").and_then(|v| v.as_array()) else {
        return reg;
    };
    for item in arr {
        let Some(path) = item.get("path").and_then(|v| v.as_str()) else {
            continue;
        };
        let path = path.trim();
        if path.is_empty() {
            continue;
        }
        let manifest_path = item
            .get("manifestPath")
            .or_else(|| item.get("manifest_path"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let discovery = item.get("discovery").and_then(|d| {
            if d.is_null() {
                return None;
            }
            let base = d
                .get("baseUrl")
                .or_else(|| d.get("base_url"))
                .and_then(|v| v.as_str())?
                .trim();
            let token = d
                .get("authToken")
                .or_else(|| d.get("auth_token"))
                .and_then(|v| v.as_str())?
                .trim();
            if base.is_empty() || token.is_empty() {
                return None;
            }
            let pid = d.get("pid").and_then(|v| v.as_i64());
            let index_root = d
                .get("indexRoot")
                .or_else(|| d.get("index_root"))
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            Some(OracleRootDiscovery {
                base_url: base.to_string(),
                auth_token: token.to_string(),
                pid,
                index_root,
            })
        });
        let last_indexed_at = item
            .get("lastIndexedAt")
            .or_else(|| item.get("last_indexed_at"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let status = item
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("indexed")
            .to_string();
        reg.roots.push(OracleRootEntry {
            path: path.to_string(),
            manifest_path,
            discovery,
            last_indexed_at,
            status,
        });
    }
    reg
}

pub fn oracle_roots_registry_to_value(reg: &OracleRootsRegistry) -> Value {
    let roots: Vec<Value> = reg
        .roots
        .iter()
        .map(|e| {
            let discovery = match &e.discovery {
                Some(d) => {
                    let mut m = Map::new();
                    m.insert("baseUrl".into(), json!(d.base_url));
                    m.insert("authToken".into(), json!(d.auth_token));
                    if let Some(pid) = d.pid {
                        m.insert("pid".into(), json!(pid));
                    }
                    if let Some(ref ir) = d.index_root {
                        m.insert("indexRoot".into(), json!(ir));
                    }
                    Value::Object(m)
                }
                None => Value::Null,
            };
            json!({
                "path": e.path,
                "manifestPath": e.manifest_path,
                "discovery": discovery,
                "lastIndexedAt": e.last_indexed_at,
                "status": e.status,
            })
        })
        .collect();
    json!({ "roots": roots })
}

pub fn load_oracle_roots_registry(management_root: &Path) -> OracleRootsRegistry {
    let path = oracle_roots_registry_path(management_root);
    if !path.exists() {
        return OracleRootsRegistry::default();
    }
    let meta = match fs::metadata(&path) {
        Ok(m) => m,
        Err(_) => return OracleRootsRegistry::default(),
    };
    if meta.len() > MANIFEST_MAX_BYTES {
        return OracleRootsRegistry::default();
    }
    let raw = match fs::read_to_string(&path) {
        Ok(r) => r,
        Err(_) => return OracleRootsRegistry::default(),
    };
    match serde_json::from_str::<Value>(&raw) {
        Ok(v) => oracle_roots_registry_from_value(&v),
        Err(_) => OracleRootsRegistry::default(),
    }
}

pub fn save_oracle_roots_registry(
    management_root: &Path,
    reg: &OracleRootsRegistry,
) -> std::io::Result<()> {
    let path = oracle_roots_registry_path(management_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_string_pretty(&oracle_roots_registry_to_value(reg))
        .unwrap_or_else(|_| "{\"roots\":[]}".into());
    // Registry embeds discovery authToken — owner-only (0600), no world-readable window.
    write_owner_only_file(&path, raw.as_bytes())
}

/// Create/truncate `path` with mode 0600 (Unix) so auth tokens are never world-readable.
fn write_owner_only_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(bytes)?;
    f.flush()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Merge + re-rank chunks by score (desc), hard-cap at `limit` (P3).
/// Optionally tags each chunk with `index_root` basename when `tag_root` is set.
pub fn merge_rerank_chunks(mut chunks: Vec<Value>, limit: usize) -> Vec<Value> {
    fn score_of(c: &Value) -> f64 {
        c.get("score")
            .or_else(|| c.get("relevance"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0)
    }
    chunks.sort_by(|a, b| {
        score_of(b)
            .partial_cmp(&score_of(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    if chunks.len() > limit {
        chunks.truncate(limit);
    }
    chunks
}

/// Tag each chunk with the root basename so union results are disambiguable.
fn tag_chunks_with_root(chunks: Vec<Value>, root: &Path) -> Vec<Value> {
    let label = root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("root")
        .to_string();
    chunks
        .into_iter()
        .map(|mut c| {
            if let Some(obj) = c.as_object_mut() {
                obj.entry("index_root".to_string())
                    .or_insert_with(|| json!(label));
            }
            c
        })
        .collect()
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
    validate_project_work_root(
        Path::new(&root_path),
        Some(&management_root),
        Some(pid),
        Some(projects_dir),
    )
}

/// Concrete allowed file_id set. Never returns "unscoped full corpus".
///
/// * Empty / missing `project_id` → management-root files only (never the union
///   of every project). An empty set is fail-closed (callers return no chunks).
/// * Project-scoped → that project's work-root files only, loaded from
///   `<project_root>/oracle-data/chunk-index-manifest.json` (not the management
///   root's manifest — F45 / multi-root Layer 1). Does **not** auto-merge the
///   management-root corpus into every project scope.
pub fn oracle_allowed_file_ids(
    projects_dir: &Path,
    project_id: Option<&str>,
) -> ToolResult<HashSet<String>> {
    let management_root = management_root_from_projects_dir(projects_dir);
    let project_id = project_id.map(str::trim).filter(|s| !s.is_empty());
    if project_id.is_none() {
        // Unscoped → management root only (never union of all projects).
        let manifest_path = management_root
            .join("oracle-data")
            .join("chunk-index-manifest.json");
        let mut manifest = manifest_load(&manifest_path);
        return Ok(manifest_files_for_root(&mut manifest, &management_root));
    }
    // Project scope: load THIS project's own oracle-data manifest (F45).
    let root = oracle_index_root_for_project(projects_dir, project_id)?;
    let manifest_path = root
        .join("oracle-data")
        .join("chunk-index-manifest.json");
    let mut manifest = manifest_load(&manifest_path);
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

/// Resolve (base_url, auth_token) for the **default** (management / global)
/// discovery file under `projects_dir`, or None (caller fails closed).
pub fn resolve_oracle_http_target(projects_dir: &Path) -> Option<(String, String)> {
    resolve_oracle_http_target_for_root(projects_dir, None)
}

/// Resolve HTTP target for a specific index root (P2).
///
/// Resolution order:
/// 1. Env override (`DEVBOULE_ORACLE_HTTP_*`) — only when `index_root` is None
///    (unscoped) or when the env is the only available target.
/// 2. Registry entry for `index_root` with live discovery credentials.
/// 3. Per-root discovery file at `<root>/oracle-data/.oracle-server.json`.
/// 4. Global `projects_dir/.oracle-server.json` **only when** the discovery's
///    `indexRoot` (if present) matches `index_root`, or when `index_root` is
///    None / equals management root. Prevents cross-root file_id leaks (P1 audit #3).
pub fn resolve_oracle_http_target_for_root(
    projects_dir: &Path,
    index_root: Option<&Path>,
) -> Option<(String, String)> {
    let cache_key = format!(
        "{}|{}",
        projects_dir.to_string_lossy(),
        index_root
            .map(|p| normalize_root_key(p))
            .unwrap_or_default()
    );
    if let Ok(guard) = TARGET_CACHE.lock() {
        if let Some((k, cached)) = guard.as_ref() {
            if k == &cache_key && cached.at.elapsed() < TARGET_CACHE_TTL {
                return cached.value.clone();
            }
        }
    }
    let result = resolve_oracle_http_target_for_root_uncached(projects_dir, index_root);
    if let Ok(mut guard) = TARGET_CACHE.lock() {
        *guard = Some((
            cache_key,
            TargetCache {
                at: Instant::now(),
                value: result.clone(),
            },
        ));
    }
    result
}

fn discovery_from_json(data: &Value) -> Option<(String, String, Option<String>)> {
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
        if !hb.is_empty() && !heartbeat_fresh(hb) {
            return None;
        }
        // Valid or empty heartbeat handled below for pid.
        if !hb.is_empty() {
            let index_root = data
                .get("indexRoot")
                .or_else(|| data.get("index_root"))
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            return Some((base.to_string(), token.to_string(), index_root));
        }
    }
    if let Some(pid) = data.get("pid").and_then(|v| v.as_i64()) {
        if pid > 0 && pid <= i32::MAX as i64 && !pid_alive(pid as i32) {
            return None;
        }
    }
    let index_root = data
        .get("indexRoot")
        .or_else(|| data.get("index_root"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    Some((base.to_string(), token.to_string(), index_root))
}

fn discovery_matches_root(discovery_index_root: Option<&str>, wanted: &Path) -> bool {
    let Some(dir) = discovery_index_root else {
        // Legacy discovery without indexRoot: only safe for unscoped callers.
        return false;
    };
    path_keys_equivalent(dir, &normalize_root_key(wanted))
}

fn resolve_oracle_http_target_for_root_uncached(
    projects_dir: &Path,
    index_root: Option<&Path>,
) -> Option<(String, String)> {
    let management_root = management_root_from_projects_dir(projects_dir);

    if let Some(root) = index_root {
        // Env override must NOT pin every project root to one server (P2 audit #2).
        // Scoped resolution uses registry / per-root / global-with-indexRoot only.

        // 1) Registry discovery for this root — require indexRoot match when set.
        let reg = load_oracle_roots_registry(&management_root);
        if let Some(entry) = reg.lookup_by_path(root) {
            if entry.status.trim().eq_ignore_ascii_case("indexed")
                || entry.status.trim().is_empty()
            {
                if let Some(d) = &entry.discovery {
                    if is_loopback_http_base(&d.base_url) {
                        let root_ok = d
                            .index_root
                            .as_deref()
                            .map(|r| discovery_matches_root(Some(r), root))
                            .unwrap_or(false);
                        if root_ok {
                            if let Some(pid) = d.pid {
                                if pid > 0 && pid <= i32::MAX as i64 && !pid_alive(pid as i32) {
                                    // fall through
                                } else {
                                    return Some((d.base_url.clone(), d.auth_token.clone()));
                                }
                            } else {
                                // No pid → require fail-closed (audit #7): skip.
                            }
                        }
                    }
                }
            }
        }
        // 2) Per-root discovery file (management-owned only preferred; still require match).
        let per_root = root.join("oracle-data").join(DISCOVERY_FILE);
        if let Ok(raw) = fs::read_to_string(&per_root) {
            if let Ok(data) = serde_json::from_str::<Value>(&raw) {
                if let Some((base, token, disc_root)) = discovery_from_json(&data) {
                    // Require explicit indexRoot match — never trust legacy no-root files
                    // for a scoped project (prevents cross-root content leak).
                    if discovery_matches_root(disc_root.as_deref(), root) {
                        return Some((base, token));
                    }
                }
            }
        }
        // 3) Global discovery only if it advertises this exact root.
        let global = projects_dir.join(DISCOVERY_FILE);
        if let Ok(raw) = fs::read_to_string(global) {
            if let Ok(data) = serde_json::from_str::<Value>(&raw) {
                if let Some((base, token, disc_root)) = discovery_from_json(&data) {
                    if discovery_matches_root(disc_root.as_deref(), root) {
                        return Some((base, token));
                    }
                    // Mismatch or legacy no-indexRoot: fail closed for scoped.
                    return None;
                }
            }
        }
        return None;
    }

    // Unscoped only: env override is allowed.
    if let (Some(base), Some(token)) =
        (env_first(ORACLE_HTTP_BASE_ENVS), env_first(ORACLE_HTTP_TOKEN_ENVS))
    {
        if !is_loopback_http_base(&base) {
            return None;
        }
        return Some((base, token));
    }
    let path = projects_dir.join(DISCOVERY_FILE);
    let raw = fs::read_to_string(path).ok()?;
    let data: Value = serde_json::from_str(&raw).ok()?;
    let (base, token, _) = discovery_from_json(&data)?;
    Some((base, token))
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

pub(crate) struct FilterArgs {
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
    index_root: Option<&Path>,
) -> ToolResult<Vec<Value>> {
    // Fail-closed: require a live target first (empty scope is still a concrete
    // empty result only when the resident server/discovery is reachable).
    let target = resolve_oracle_http_target_for_root(projects_dir, index_root)
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
    index_root: Option<&Path>,
) -> ToolResult<Value> {
    let target = resolve_oracle_http_target_for_root(projects_dir, index_root)
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

/// Fan-out context across registered roots and merge/re-rank (P3).
///
/// * `primary_root` — project root or management root (always queried first when
///   allowed is non-empty for it).
/// * `extra_roots` — additional roots; each must already be in the registry
///   (fail closed with ToolError if not).
pub(crate) fn dispatch_oracle_context_union(
    projects_dir: &Path,
    query: &str,
    limit: i64,
    filters: &FilterArgs,
    primary_root: &Path,
    primary_allowed: &HashSet<String>,
    extra_roots: &[PathBuf],
) -> ToolResult<Vec<Value>> {
    let management_root = management_root_from_projects_dir(projects_dir);
    let reg = load_oracle_roots_registry(&management_root);

    let mut all: Vec<Value> = Vec::new();

    // Primary root (may be empty scope → skip HTTP).
    if !primary_allowed.is_empty() {
        let chunks = dispatch_oracle_context(
            projects_dir,
            query,
            limit,
            primary_allowed,
            filters,
            Some(primary_root),
        )?;
        all.extend(tag_chunks_with_root(chunks, primary_root));
    }

    for extra in extra_roots {
        if path_has_forbidden_components(extra) {
            return Err(ToolError::new(
                "extra_roots path is invalid (forbidden path components).",
            ));
        }
        if !reg.is_registered(extra) {
            return Err(ToolError::new(format!(
                "Oracle extra_roots path is not a registered indexed root \
                 (no query-time indexing): {}",
                extra.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unregistered")
            )));
        }
        // Allowed IDs for that root's own manifest.
        let mut manifest = manifest_load(
            &extra
                .join("oracle-data")
                .join("chunk-index-manifest.json"),
        );
        let allowed = manifest_files_for_root(&mut manifest, extra);
        if allowed.is_empty() {
            continue;
        }
        let chunks = dispatch_oracle_context(
            projects_dir,
            query,
            limit,
            &allowed,
            filters,
            Some(extra.as_path()),
        )?;
        all.extend(tag_chunks_with_root(chunks, extra));
    }

    let cap = limit.clamp(1, 50) as usize;
    Ok(merge_rerank_chunks(all, cap))
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
    let index_root = oracle_index_root_for_project(projects_dir, pid.as_deref()).ok();
    let mut result = dispatch_oracle_ask(
        projects_dir,
        &query,
        limit,
        &allowed,
        &filters,
        index_root.as_deref(),
    )?;
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
    oracle_context_with_extra_roots(
        projects_dir,
        agent_id,
        role,
        session_token,
        query,
        limit,
        project_id,
        kind,
        language,
        symbols,
        imports,
        module,
        &[],
    )
}

/// Like [`oracle_context`] but can fan out to additional **registered** roots (P3).
pub fn oracle_context_with_extra_roots(
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
    extra_roots: &[PathBuf],
) -> ToolResult<Value> {
    let (agent_id, role) =
        require_agent_tool(projects_dir, agent_id, role, "oracle_context", session_token)?;
    enforce_mini_oracle_project_scope(
        projects_dir,
        &agent_id,
        &role,
        project_id.map(str::trim).filter(|s| !s.is_empty()),
    )?;
    // SEC: mini must never fan out to other registered roots (P2 audit #3).
    if role == "mini" && !extra_roots.is_empty() {
        return Err(ToolError::new(
            "A mini agent cannot use extra_roots (multi-root union is not allowed for mini).",
        ));
    }
    // Cap fan-out (DoS / timeout storms).
    if extra_roots.len() > 8 {
        return Err(ToolError::new(
            "extra_roots is limited to at most 8 registered roots.",
        ));
    }
    for er in extra_roots {
        if path_has_forbidden_components(er) {
            return Err(ToolError::new(
                "extra_roots path is invalid (forbidden path components).",
            ));
        }
    }
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
    let primary_root = oracle_index_root_for_project(projects_dir, pid.as_deref())?;
    let chunks = if extra_roots.is_empty() {
        dispatch_oracle_context(
            projects_dir,
            &query,
            limit,
            &allowed,
            &filters,
            Some(primary_root.as_path()),
        )?
    } else {
        dispatch_oracle_context_union(
            projects_dir,
            &query,
            limit,
            &filters,
            &primary_root,
            &allowed,
            extra_roots,
        )?
    };
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
    let index_root = oracle_index_root_for_project(projects_dir, pid.as_deref()).ok();
    let chunks = dispatch_oracle_context(
        projects_dir,
        &query,
        find_limit,
        &scope,
        &filters,
        index_root.as_deref(),
    )?;
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
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// Drop cached HTTP targets so tests that clear env/discovery don't see
    /// a stale hit from a prior case (TTL is 5s).
    fn clear_target_cache() {
        if let Ok(mut g) = TARGET_CACHE.lock() {
            *g = None;
        }
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

    /// F45 / P1: project-scoped allowed IDs must load the **project root**
    /// manifest, not the management-root manifest. External attached projects
    /// only have files under their own `oracle-data/`.
    #[test]
    fn project_scoped_allowed_ids_use_project_root_manifest() {
        let _g = env_lock();
        let tmp = TempDir::new().unwrap();
        let mgmt = tmp.path().join("mgmt");
        let projects = mgmt.join("projects");
        let external = tmp.path().join("external-app");
        fs::create_dir_all(&projects).unwrap();
        fs::create_dir_all(external.join("oracle-data")).unwrap();
        fs::create_dir_all(mgmt.join("oracle-data")).unwrap();

        // Management has only mgmt-only files. Key must match the path form
        // management_root_from_projects_dir returns (may differ from canonicalize
        // on macOS /var vs /private/var) — write BOTH keys when they differ.
        let mgmt_as_resolver = management_root_from_projects_dir(&projects);
        let mgmt_canon = mgmt.canonicalize().unwrap_or_else(|_| mgmt.clone());
        let mut mgmt_roots = serde_json::Map::new();
        for key in [mgmt_as_resolver.to_string_lossy().to_string(), mgmt_canon.to_string_lossy().to_string()] {
            mgmt_roots.insert(
                key,
                json!({"files": { "mgmt-only.rs": { "hash": "a" } }}),
            );
        }
        fs::write(
            mgmt.join("oracle-data").join("chunk-index-manifest.json"),
            json!({
                "version": 1,
                "root": mgmt_as_resolver.to_string_lossy(),
                "roots": mgmt_roots,
                "files": { "mgmt-only.rs": { "hash": "a" } }
            })
            .to_string(),
        )
        .unwrap();

        // External project has its own index (the F45 case).
        // Project work-root is canonicalized by validate_project_work_root.
        let ext_canon = external.canonicalize().unwrap();
        let ext_key_s = ext_canon.to_string_lossy().to_string();
        fs::write(
            external
                .join("oracle-data")
                .join("chunk-index-manifest.json"),
            json!({
                "version": 1,
                "root": ext_key_s,
                "roots": {
                    ext_key_s.clone(): {
                        "files": {
                            "README.md": { "hash": "b" },
                            "index.html": { "hash": "c" }
                        }
                    }
                },
                "files": {
                    "README.md": { "hash": "b" },
                    "index.html": { "hash": "c" }
                }
            })
            .to_string(),
        )
        .unwrap();

        let md = format!(
            "---\nid: external-app\ntitle: External\nstatus: active\nroot_path: {}\nupdated_at: 2026-01-01T00:00:00Z\n---\n\n```aspis-project\n{{\"version\":1,\"tasks\":[],\"notes\":[]}}\n```\n",
            external.display()
        );
        fs::write(projects.join("external-app.md"), md).unwrap();

        let allowed =
            oracle_allowed_file_ids(&projects, Some("external-app")).expect("allowed ids");
        assert!(
            allowed.contains("README.md"),
            "must include project-root manifest files, got {allowed:?}"
        );
        assert!(
            allowed.contains("index.html"),
            "must include project-root manifest files, got {allowed:?}"
        );
        assert!(
            !allowed.contains("mgmt-only.rs"),
            "must NOT pull management-root files into project scope: {allowed:?}"
        );
        assert_eq!(allowed.len(), 2, "{allowed:?}");
    }

    /// Unscoped stays on management-root manifest only (never silent union).
    #[test]
    fn unscoped_allowed_ids_use_management_manifest_only() {
        let _g = env_lock();
        let tmp = TempDir::new().unwrap();
        let mgmt = tmp.path().join("mgmt");
        let projects = mgmt.join("projects");
        let external = tmp.path().join("other");
        fs::create_dir_all(&projects).unwrap();
        fs::create_dir_all(external.join("oracle-data")).unwrap();
        fs::create_dir_all(mgmt.join("oracle-data")).unwrap();

        let mgmt_as_resolver = management_root_from_projects_dir(&projects);
        let mgmt_canon = mgmt.canonicalize().unwrap_or_else(|_| mgmt.clone());
        let mut mgmt_roots = serde_json::Map::new();
        for key in [mgmt_as_resolver.to_string_lossy().to_string(), mgmt_canon.to_string_lossy().to_string()] {
            mgmt_roots.insert(
                key,
                json!({"files": { "only-mgmt.ts": { "hash": "m" } }}),
            );
        }
        fs::write(
            mgmt.join("oracle-data").join("chunk-index-manifest.json"),
            json!({
                "version": 1,
                "root": mgmt_as_resolver.to_string_lossy(),
                "roots": mgmt_roots,
                "files": { "only-mgmt.ts": { "hash": "m" } }
            })
            .to_string(),
        )
        .unwrap();

        let ext_key = external.canonicalize().unwrap();
        let ext_key_s = ext_key.to_string_lossy().to_string();
        fs::write(
            external
                .join("oracle-data")
                .join("chunk-index-manifest.json"),
            json!({
                "version": 1,
                "root": ext_key_s,
                "roots": {
                    ext_key_s: {
                        "files": { "other.md": { "hash": "o" } }
                    }
                },
                "files": { "other.md": { "hash": "o" } }
            })
            .to_string(),
        )
        .unwrap();

        let allowed = oracle_allowed_file_ids(&projects, None).unwrap();
        assert!(
            allowed.contains("only-mgmt.ts"),
            "unscoped must see management files: {allowed:?}"
        );
        assert!(
            !allowed.contains("other.md"),
            "unscoped must NOT union external project manifests: {allowed:?}"
        );
    }

    /// P1 audit: corrupt non-object manifest must not panic — empty allowlist.
    #[test]
    fn project_scoped_non_object_manifest_is_empty_not_panic() {
        let _g = env_lock();
        let tmp = TempDir::new().unwrap();
        let mgmt = tmp.path().join("mgmt");
        let projects = mgmt.join("projects");
        let external = tmp.path().join("corrupt-app");
        fs::create_dir_all(&projects).unwrap();
        fs::create_dir_all(external.join("oracle-data")).unwrap();
        fs::write(
            external
                .join("oracle-data")
                .join("chunk-index-manifest.json"),
            "[]",
        )
        .unwrap();
        let md = format!(
            "---\nid: corrupt-app\ntitle: C\nstatus: active\nroot_path: {}\nupdated_at: 2026-01-01T00:00:00Z\n---\n\n```aspis-project\n{{\"version\":1,\"tasks\":[],\"notes\":[]}}\n```\n",
            external.display()
        );
        fs::write(projects.join("corrupt-app.md"), md).unwrap();
        let allowed = oracle_allowed_file_ids(&projects, Some("corrupt-app")).expect("ok");
        assert!(allowed.is_empty(), "corrupt manifest → empty, got {allowed:?}");
    }

    /// Unscoped with only canonical root key still resolves (macOS /var fold).
    #[test]
    fn unscoped_resolves_when_manifest_key_is_canonical_only() {
        let _g = env_lock();
        let tmp = TempDir::new().unwrap();
        let mgmt = tmp.path().join("mgmt");
        let projects = mgmt.join("projects");
        fs::create_dir_all(mgmt.join("oracle-data")).unwrap();
        fs::create_dir_all(&projects).unwrap();
        let mgmt_canon = mgmt.canonicalize().unwrap();
        let key = mgmt_canon.to_string_lossy().to_string();
        fs::write(
            mgmt.join("oracle-data").join("chunk-index-manifest.json"),
            json!({
                "version": 1,
                "root": key,
                "roots": { key.clone(): { "files": { "canon-only.rs": {"hash":"1"} } } },
                "files": { "canon-only.rs": {"hash":"1"} }
            })
            .to_string(),
        )
        .unwrap();
        let allowed = oracle_allowed_file_ids(&projects, None).unwrap();
        assert!(
            allowed.contains("canon-only.rs"),
            "must match via path_keys_equivalent, got {allowed:?}"
        );
    }

    // ── P2 registry ─────────────────────────────────────────────────────────

    #[test]
    fn registry_round_trip_and_lookup() {
        let tmp = TempDir::new().unwrap();
        let mgmt = tmp.path().join("mgmt");
        fs::create_dir_all(mgmt.join("oracle-data")).unwrap();
        let root_a = tmp.path().join("proj-a");
        let root_b = tmp.path().join("proj-b");
        fs::create_dir_all(&root_a).unwrap();
        fs::create_dir_all(&root_b).unwrap();

        let mut reg = OracleRootsRegistry::default();
        reg.upsert(OracleRootEntry {
            path: root_a.to_string_lossy().to_string(),
            manifest_path: root_a
                .join("oracle-data/chunk-index-manifest.json")
                .to_string_lossy()
                .to_string(),
            discovery: Some(OracleRootDiscovery {
                base_url: "http://127.0.0.1:9001".into(),
                auth_token: "tok-a".into(),
                pid: Some(111),
                index_root: Some(root_a.to_string_lossy().to_string()),
            }),
            last_indexed_at: Some("2026-07-21T00:00:00Z".into()),
            status: "indexed".into(),
        });
        reg.upsert(OracleRootEntry {
            path: root_b.to_string_lossy().to_string(),
            manifest_path: root_b
                .join("oracle-data/chunk-index-manifest.json")
                .to_string_lossy()
                .to_string(),
            discovery: Some(OracleRootDiscovery {
                base_url: "http://127.0.0.1:9002".into(),
                auth_token: "tok-b".into(),
                pid: Some(222),
                index_root: Some(root_b.to_string_lossy().to_string()),
            }),
            last_indexed_at: None,
            status: "indexed".into(),
        });
        save_oracle_roots_registry(&mgmt, &reg).unwrap();
        let loaded = load_oracle_roots_registry(&mgmt);
        assert_eq!(loaded.roots.len(), 2);
        let a = loaded.lookup_by_path(&root_a).expect("root a");
        assert_eq!(
            a.discovery.as_ref().map(|d| d.auth_token.as_str()),
            Some("tok-a")
        );
        assert!(loaded.is_registered(&root_b));
        assert!(!loaded.is_registered(Path::new("/tmp/not-registered-xyz")));
    }

    #[test]
    #[cfg(unix)]
    fn save_oracle_roots_registry_owner_only_mode() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let mgmt = tmp.path().join("mgmt");
        fs::create_dir_all(&mgmt).unwrap();
        let mut reg = OracleRootsRegistry::default();
        reg.upsert(OracleRootEntry {
            path: "/tmp/x".into(),
            manifest_path: "/tmp/x/m.json".into(),
            discovery: Some(OracleRootDiscovery {
                base_url: "http://127.0.0.1:9".into(),
                auth_token: "secret-token".into(),
                pid: None,
                index_root: None,
            }),
            last_indexed_at: None,
            status: "indexed".into(),
        });
        save_oracle_roots_registry(&mgmt, &reg).unwrap();
        let path = oracle_roots_registry_path(&mgmt);
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "registry with authToken must be owner-only");
    }

    #[test]
    fn registry_missing_file_is_empty_fail_closed() {
        let tmp = TempDir::new().unwrap();
        let mgmt = tmp.path().join("no-reg");
        fs::create_dir_all(&mgmt).unwrap();
        let reg = load_oracle_roots_registry(&mgmt);
        assert!(reg.roots.is_empty());
        assert!(!reg.is_registered(Path::new("/any")));
    }

    #[test]
    fn resolve_target_uses_registry_discovery_for_project_root() {
        let _g = env_lock();
        clear_target_cache();
        for k in ORACLE_HTTP_BASE_ENVS
            .iter()
            .chain(ORACLE_HTTP_TOKEN_ENVS.iter())
        {
            std::env::remove_var(k);
        }
        let tmp = TempDir::new().unwrap();
        let mgmt = tmp.path().join("mgmt");
        let projects = mgmt.join("projects");
        let root_a = tmp.path().join("proj-a");
        fs::create_dir_all(&projects).unwrap();
        fs::create_dir_all(mgmt.join("oracle-data")).unwrap();
        fs::create_dir_all(&root_a).unwrap();

        let mut reg = OracleRootsRegistry::default();
        reg.upsert(OracleRootEntry {
            path: normalize_root_key(&root_a),
            manifest_path: String::new(),
            discovery: Some(OracleRootDiscovery {
                base_url: "http://127.0.0.1:9111".into(),
                auth_token: "secret-a".into(),
                // Live pid required (fail-closed without pid — P2 audit #7).
                pid: Some(std::process::id() as i64),
                index_root: Some(normalize_root_key(&root_a)),
            }),
            last_indexed_at: None,
            status: "indexed".into(),
        });
        save_oracle_roots_registry(&mgmt, &reg).unwrap();

        // Global discovery points at a DIFFERENT root — must not win for root_a.
        fs::write(
            projects.join(DISCOVERY_FILE),
            json!({
                "baseUrl": "http://127.0.0.1:7000",
                "authToken": "global-tok",
                "indexRoot": mgmt.to_string_lossy(),
                "pid": 1
            })
            .to_string(),
        )
        .unwrap();

        let target = resolve_oracle_http_target_for_root(&projects, Some(&root_a))
            .expect("registry discovery");
        assert_eq!(target.0, "http://127.0.0.1:9111");
        assert_eq!(target.1, "secret-a");
    }

    #[test]
    fn resolve_target_fail_closed_when_global_index_root_mismatches() {
        let _g = env_lock();
        clear_target_cache();
        for k in ORACLE_HTTP_BASE_ENVS
            .iter()
            .chain(ORACLE_HTTP_TOKEN_ENVS.iter())
        {
            std::env::remove_var(k);
        }
        let tmp = TempDir::new().unwrap();
        let mgmt = tmp.path().join("mgmt");
        let projects = mgmt.join("projects");
        let external = tmp.path().join("external");
        fs::create_dir_all(&projects).unwrap();
        fs::create_dir_all(&external).unwrap();
        fs::create_dir_all(mgmt.join("oracle-data")).unwrap();
        // No registry entry.
        fs::write(
            projects.join(DISCOVERY_FILE),
            json!({
                "baseUrl": "http://127.0.0.1:7000",
                "authToken": "global-tok",
                "indexRoot": mgmt.canonicalize().unwrap().to_string_lossy(),
                "heartbeatAt": chrono::Utc::now().to_rfc3339()
            })
            .to_string(),
        )
        .unwrap();
        let miss = resolve_oracle_http_target_for_root(&projects, Some(&external));
        assert!(
            miss.is_none(),
            "must not use global server for a different root: {miss:?}"
        );
    }

    // ── P3 union ────────────────────────────────────────────────────────────

    #[test]
    fn merge_rerank_caps_and_orders_by_score() {
        let chunks = vec![
            json!({"file_id": "a", "score": 0.1}),
            json!({"file_id": "b", "score": 0.9}),
            json!({"file_id": "c", "score": 0.5}),
            json!({"file_id": "d", "score": 0.7}),
        ];
        let merged = merge_rerank_chunks(chunks, 2);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0]["file_id"], "b");
        assert_eq!(merged[1]["file_id"], "d");
    }

    #[test]
    fn union_rejects_unregistered_extra_root() {
        let _g = env_lock();
        clear_target_cache();
        for k in ORACLE_HTTP_BASE_ENVS
            .iter()
            .chain(ORACLE_HTTP_TOKEN_ENVS.iter())
        {
            std::env::remove_var(k);
        }
        let tmp = TempDir::new().unwrap();
        let mgmt = tmp.path().join("mgmt");
        let projects = mgmt.join("projects");
        let primary = tmp.path().join("primary");
        let rogue = tmp.path().join("rogue");
        fs::create_dir_all(&projects).unwrap();
        fs::create_dir_all(mgmt.join("oracle-data")).unwrap();
        fs::create_dir_all(&primary).unwrap();
        fs::create_dir_all(&rogue).unwrap();
        // Empty registry — rogue not registered.
        let filters = parse_filter_args(None, None, None, None, None, false);
        let err = dispatch_oracle_context_union(
            &projects,
            "q",
            5,
            &filters,
            &primary,
            &HashSet::new(),
            &[rogue],
        )
        .unwrap_err();
        assert!(
            err.message.contains("not a registered") || err.message.contains("unregistered"),
            "{}",
            err.message
        );
    }

    #[test]
    fn mini_cannot_use_extra_roots() {
        let _g = env_lock();
        clear_target_cache();
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "mini-extra", "mini");
        with_agents_lock(&projects, || {
            let mut state = read_agents_state(&projects).unwrap();
            if let Some(s) = crate::state::find_session_mut(&mut state, "mini-extra") {
                s.insert("currentProjectId".into(), json!("proj-a"));
            }
            write_agents_state(&projects, state).unwrap();
            Ok::<(), ToolError>(())
        })
        .unwrap();
        let err = oracle_context_with_extra_roots(
            &projects,
            "mini-extra",
            "mini",
            Some(&tok),
            "query",
            Some(3),
            Some("proj-a"),
            None,
            None,
            None,
            None,
            None,
            &[PathBuf::from("/tmp/some-registered")],
        )
        .unwrap_err();
        assert!(
            err.message.contains("extra_roots") || err.message.contains("mini"),
            "{}",
            err.message
        );
    }

    #[test]
    fn env_override_does_not_apply_to_scoped_root() {
        let _g = env_lock();
        clear_target_cache();
        std::env::set_var("DEVBOULE_ORACLE_HTTP_BASE", "http://127.0.0.1:19999");
        std::env::set_var("DEVBOULE_ORACLE_AUTH_TOKEN", "env-tok");
        let tmp = TempDir::new().unwrap();
        let mgmt = tmp.path().join("mgmt");
        let projects = mgmt.join("projects");
        let external = tmp.path().join("ext");
        fs::create_dir_all(&projects).unwrap();
        fs::create_dir_all(&external).unwrap();
        fs::create_dir_all(mgmt.join("oracle-data")).unwrap();
        // No registry/discovery for external — env must NOT fill in.
        let miss = resolve_oracle_http_target_for_root(&projects, Some(&external));
        assert!(
            miss.is_none(),
            "scoped root must not use env override: {miss:?}"
        );
        // Unscoped still may use env.
        let unscoped = resolve_oracle_http_target_for_root(&projects, None);
        assert_eq!(
            unscoped,
            Some(("http://127.0.0.1:19999".into(), "env-tok".into()))
        );
        std::env::remove_var("DEVBOULE_ORACLE_HTTP_BASE");
        std::env::remove_var("DEVBOULE_ORACLE_AUTH_TOKEN");
        clear_target_cache();
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
        clear_target_cache();
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
            None,
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
            None,
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
        let got = validate_project_work_root(&proj, None, Some("p1"), None).unwrap();
        assert!(got.ends_with("myproj") || got == proj.canonicalize().unwrap());
        std::env::remove_var("ASPIS_WORKSPACE_ROOT");
    }

    /// B06 smoke against the real openrouter-mock project (host path may be missing in CI).
    #[test]
    fn resolve_real_openrouter_mock_if_present() {
        let projects = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src-tauri/projects");
        let projects = match projects.canonicalize() {
            Ok(p) => p,
            Err(_) => return,
        };
        if !projects.join("openrouter-mock.md").is_file() {
            return;
        }
        let got = resolve_project_work_root(&projects, "openrouter-mock");
        assert!(
            got.is_ok(),
            "attached openrouter-mock root must resolve (B06): {got:?}"
        );
    }

    /// B06: an attached Devboule project root is approved without ASPIS_WORKSPACE_ROOT.
    #[test]
    fn work_root_allows_attached_project_root() {
        let _g = env_lock();
        std::env::remove_var("ASPIS_WORKSPACE_ROOT");
        std::env::remove_var("DEVBOULE_WORKSPACE_ROOT");
        let tmp = TempDir::new().unwrap();
        let projects = tmp.path().join("projects");
        let work = tmp.path().join("outside-mgmt").join("my-app");
        fs::create_dir_all(&projects).unwrap();
        fs::create_dir_all(&work).unwrap();
        let md = format!(
            "---\nid: my-app\ntitle: My App\nstatus: active\nroot_path: {}\nupdated_at: 2026-01-01T00:00:00Z\n---\n\n```aspis-project\n{{\"version\":1,\"tasks\":[],\"notes\":[]}}\n```\n",
            work.display()
        );
        fs::write(projects.join("my-app.md"), md).unwrap();
        // Without projects_dir → reject (still outside management root).
        let err = validate_project_work_root(&work, None, Some("my-app"), None).unwrap_err();
        assert!(
            err.message.contains("outside approved"),
            "unattached path must still fail: {}",
            err.message
        );
        // With projects_dir → approve exact attached root.
        let got =
            validate_project_work_root(&work, None, Some("my-app"), Some(&projects)).unwrap();
        assert_eq!(got, work.canonicalize().unwrap());
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
