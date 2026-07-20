//! Project markdown parse/write (`*.md` + ````aspis-project```` JSON fence).
//!
//! Standalone parity with `oracle/server/aspis_mcp.py` (not Tauri-coupled).
//! Fence name is intentionally still `aspis-project` (rename deferred past P2).

use crate::state::{
    clean_text, now_rfc3339, with_file_lock, write_text_crash_safe, ToolError, ToolResult,
};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const BLOCK_MARKER: &str = "```aspis-project";
const BLOCK_CLOSE: &str = "```";

const VALID_TASK_STATUSES: &[&str] = &["todo", "wip", "review", "blocked", "done"];
const READABLE_PROJECT_STATUSES: &[&str] = &["active", "paused", "done", "archived", "draft"];
const VALID_TASK_CATEGORIES: &[&str] = &["feature", "hardening", "bug", "other"];

// ── ids / normalizers ───────────────────────────────────────────────────────

/// `^[a-z0-9][a-z0-9-]{1,79}$` — total length 2–80.
pub fn normalize_project_id(value: &str) -> ToolResult<String> {
    let project_id = value.trim().to_ascii_lowercase();
    let valid = project_id.len() >= 2
        && project_id.len() <= 80
        && project_id
            .chars()
            .next()
            .map(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            .unwrap_or(false)
        && project_id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid {
        return Err(ToolError::new(
            "Project id must use lowercase letters, numbers and hyphens.",
        ));
    }
    Ok(project_id)
}

pub fn normalize_task_id(value: &str) -> ToolResult<String> {
    let task_id = value.trim();
    let valid = !task_id.is_empty()
        && task_id.len() <= 40
        && task_id
            .chars()
            .next()
            .map(|c| c.is_ascii_alphabetic())
            .unwrap_or(false)
        && task_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !valid {
        return Err(ToolError::new("Task id is invalid."));
    }
    Ok(task_id.to_string())
}

pub fn normalize_task_status(value: &str) -> ToolResult<String> {
    let status = value.trim().to_ascii_lowercase();
    if !VALID_TASK_STATUSES.contains(&status.as_str()) {
        return Err(ToolError::new(
            "Task status must be todo, wip, review, blocked or done.",
        ));
    }
    Ok(status)
}

pub fn normalize_project_status(value: &str) -> ToolResult<String> {
    let status = value.trim().to_ascii_lowercase();
    if !READABLE_PROJECT_STATUSES.contains(&status.as_str()) {
        return Err(ToolError::new(
            "Project status must be active, paused, done, archived or draft.",
        ));
    }
    Ok(status)
}

pub fn normalize_task_category(value: Option<&str>) -> ToolResult<String> {
    let category = value.unwrap_or("").trim().to_ascii_lowercase();
    if category.is_empty() {
        return Ok("other".into());
    }
    if !VALID_TASK_CATEGORIES.contains(&category.as_str()) {
        return Err(ToolError::new(
            "Task category must be one of feature, hardening, bug, other.",
        ));
    }
    Ok(category)
}

/// Trim + cap 4000, preserve newlines; `None` when blank.
pub fn clean_description(value: Option<&str>) -> Option<String> {
    let text = value.unwrap_or("").trim();
    if text.is_empty() {
        return None;
    }
    Some(text.chars().take(4000).collect())
}

pub fn note_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let hex = &uuid::Uuid::new_v4().simple().to_string()[..8];
    format!("N{ns}-{hex}")
}

pub fn next_task_id(tasks: &[Value]) -> String {
    let mut max_id = 0i64;
    for task in tasks {
        let Some(id) = task.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        if let Some(rest) = id.strip_prefix('T') {
            if rest.chars().all(|c| c.is_ascii_digit()) {
                if let Ok(n) = rest.parse::<i64>() {
                    max_id = max_id.max(n);
                }
            }
        }
    }
    format!("T{}", max_id + 1)
}

// ── path confinement ────────────────────────────────────────────────────────

pub fn ensure_inside_projects(projects_dir: &Path, path: &Path) -> ToolResult<PathBuf> {
    // Fail closed: if the projects root cannot be resolved, refuse (do not
    // fall back to the unresolved path — that weakens confinement checks).
    let projects_resolved = projects_dir.canonicalize().map_err(|e| {
        ToolError::new(format!("Could not resolve projects directory: {e}"))
    })?;
    // Resolve path; if missing, resolve parent + append name.
    // canonicalize follows symlinks, so a link under projects/ that points
    // outside will resolve outside and fail the Path::starts_with check.
    let resolved = if path.exists() {
        path.canonicalize().map_err(|e| {
            ToolError::new(format!("Could not resolve project path: {e}"))
        })?
    } else {
        let parent = path.parent().unwrap_or(Path::new("."));
        let name = path.file_name().ok_or_else(|| {
            ToolError::new("Resolved project path escapes the projects folder.")
        })?;
        let parent_res = parent.canonicalize().map_err(|e| {
            ToolError::new(format!("Could not resolve project parent path: {e}"))
        })?;
        parent_res.join(name)
    };
    // Path component check only — never string prefix (would accept
    // `/…/projects_backup/x` when projects_dir is `/…/projects`).
    if !resolved.starts_with(&projects_resolved) {
        return Err(ToolError::new(
            "Resolved project path escapes the projects folder.",
        ));
    }
    Ok(resolved)
}

pub fn project_path(projects_dir: &Path, project_id: &str) -> ToolResult<PathBuf> {
    let normalized = normalize_project_id(project_id)?;
    let path = projects_dir.join(format!("{normalized}.md"));
    ensure_inside_projects(projects_dir, &path)
}

pub fn project_lock_path(projects_dir: &Path, project_id: &str) -> ToolResult<PathBuf> {
    Ok(project_path(projects_dir, project_id)?.with_extension("md.lock"))
}

// ── YAML frontmatter (simple) ───────────────────────────────────────────────

fn unquote_simple_yaml_value(value: &str) -> String {
    let v = value;
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        return v[1..v.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\");
    }
    if v.len() >= 2 && v.starts_with('\'') && v.ends_with('\'') {
        return v[1..v.len() - 1].replace("''", "'");
    }
    v.to_string()
}

fn yaml_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Ordered frontmatter fields (preserves unknown keys on rewrite).
#[derive(Debug, Clone)]
pub struct Frontmatter {
    pub fields: Vec<(String, String)>,
}

impl Frontmatter {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn set(&mut self, key: &str, value: String) {
        if let Some((_, v)) = self.fields.iter_mut().find(|(k, _)| k == key) {
            *v = value;
        } else {
            self.fields.push((key.to_string(), value));
        }
    }

    pub fn remove(&mut self, key: &str) {
        self.fields.retain(|(k, _)| k != key);
    }

    pub fn id(&self) -> &str {
        self.get("id").unwrap_or("")
    }

    pub fn title(&self) -> &str {
        self.get("title").unwrap_or_else(|| self.id())
    }

    pub fn status(&self) -> &str {
        self.get("status").unwrap_or("active")
    }

    pub fn updated_at(&self) -> String {
        self.get("updated_at")
            .or_else(|| self.get("updatedAt"))
            .unwrap_or("")
            .to_string()
    }

    pub fn root_path(&self) -> Option<String> {
        self.get("root_path")
            .or_else(|| self.get("rootPath"))
            .or_else(|| self.get("root"))
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    }

    pub fn censor_trusted(&self) -> bool {
        self.get("censor_trusted")
            .or_else(|| self.get("censorTrusted"))
            .map(|v| v.trim().eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    }
}

fn parse_simple_yaml(raw: &str) -> Frontmatter {
    let mut fields = Vec::new();
    for line in raw.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_string();
        if key.is_empty() {
            continue;
        }
        fields.push((key, unquote_simple_yaml_value(value.trim())));
    }
    Frontmatter { fields }
}

fn parse_frontmatter(content: &str, path: &Path) -> ToolResult<(Frontmatter, usize)> {
    if !content.starts_with("---") {
        return Err(ToolError::new(format!(
            "Project file {} is missing frontmatter.",
            path.display()
        )));
    }
    let first_newline = content.find('\n').ok_or_else(|| {
        ToolError::new(format!(
            "Project file {} has malformed frontmatter.",
            path.display()
        ))
    })?;
    let close_rel = content[first_newline + 1..].find("\n---").ok_or_else(|| {
        ToolError::new(format!(
            "Project file {} has unterminated frontmatter.",
            path.display()
        ))
    })?;
    let close = first_newline + 1 + close_rel;
    let close_end = match content[close + 1..].find('\n') {
        Some(n) => close + 1 + n + 1,
        None => content.len(),
    };
    let raw = &content[first_newline + 1..close];
    let mut fm = parse_simple_yaml(raw);

    let fallback_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("project");
    let canonical_id = normalize_project_id(fallback_id)?;
    let id_raw = fm.get("id").unwrap_or(fallback_id);
    let project_id = normalize_project_id(id_raw)?;
    if path.is_absolute() && path.exists() && project_id != canonical_id {
        return Err(ToolError::new(format!(
            "Project file {} has id '{}' but filename expects '{}'.",
            path.display(),
            project_id,
            canonical_id
        )));
    }
    // Normalize required fields into the map (keep unknown keys).
    fm.set("id", project_id.clone());
    let title = clean_text(fm.get("title").unwrap_or(&project_id), "Project title", 500)?;
    fm.set("title", title);
    let status = normalize_project_status(fm.get("status").unwrap_or("active"))?;
    fm.set("status", status);
    if fm.get("updated_at").is_none() && fm.get("updatedAt").is_none() {
        fm.set("updated_at", now_rfc3339());
    } else if let Some(ua) = fm.get("updatedAt").map(|s| s.to_string()) {
        // Prefer snake_case on disk when only camelCase present.
        if fm.get("updated_at").is_none() {
            fm.set("updated_at", ua);
            fm.remove("updatedAt");
        }
    }
    Ok((fm, close_end))
}

fn render_frontmatter(fm: &Frontmatter) -> String {
    let mut out = String::from("---\n");
    // Emit known keys first in stable order, then unknown keys (preserve order).
    let known = [
        "id",
        "title",
        "status",
        "updated_at",
        "root_path",
        "censor_trusted",
    ];
    let mut emitted = std::collections::HashSet::new();
    for key in known {
        // Skip aliases handled below.
        if key == "updated_at" {
            let val = fm
                .get("updated_at")
                .or_else(|| fm.get("updatedAt"))
                .unwrap_or("");
            out.push_str(&format!("updated_at: {val}\n"));
            emitted.insert("updated_at");
            emitted.insert("updatedAt");
            continue;
        }
        if key == "root_path" {
            if let Some(rp) = fm.root_path() {
                out.push_str(&format!("root_path: {}\n", yaml_quote(&rp)));
            }
            emitted.insert("root_path");
            emitted.insert("rootPath");
            emitted.insert("root");
            continue;
        }
        if key == "censor_trusted" {
            // NO-CHURN: only emit when true (matches Rust/Python serializers).
            if fm.censor_trusted() {
                out.push_str("censor_trusted: true\n");
            }
            emitted.insert("censor_trusted");
            emitted.insert("censorTrusted");
            continue;
        }
        if let Some(val) = fm.get(key) {
            out.push_str(&format!("{key}: {val}\n"));
            emitted.insert(key);
        }
    }
    for (k, v) in &fm.fields {
        if emitted.contains(k.as_str()) {
            continue;
        }
        // Preserve unknown keys (and aliases we didn't normalize away).
        if v.chars().any(|c| c.is_whitespace()) || v.contains(':') || v.contains('"') {
            out.push_str(&format!("{k}: {}\n", yaml_quote(v)));
        } else {
            out.push_str(&format!("{k}: {v}\n"));
        }
        emitted.insert(k.as_str());
    }
    out.push_str("---\n");
    out
}

// ── state block ─────────────────────────────────────────────────────────────

fn find_state_block(content: &str) -> ToolResult<(Value, (usize, usize))> {
    let start = content.find(BLOCK_MARKER).ok_or_else(|| {
        ToolError::new("Project file is missing ```aspis-project block.")
    })?;
    let body_start = content[start..]
        .find('\n')
        .map(|n| start + n + 1)
        .ok_or_else(|| ToolError::new("Project state block is malformed."))?;
    let mut cursor = body_start;
    for line in content[body_start..].split_inclusive('\n') {
        let line_start = cursor;
        cursor += line.len();
        if line.trim() == BLOCK_CLOSE {
            let body = content[body_start..line_start].trim();
            let mut state: Value = if body.is_empty() {
                json!({"version": 1, "tasks": [], "notes": []})
            } else {
                serde_json::from_str(body).map_err(|e| {
                    ToolError::new(format!("Project state JSON is invalid: {e}"))
                })?
            };
            if let Some(obj) = state.as_object_mut() {
                obj.entry("tasks".to_string()).or_insert_with(|| json!([]));
                obj.entry("notes".to_string()).or_insert_with(|| json!([]));
            }
            validate_project_state(&state)?;
            return Ok((state, (start, cursor)));
        }
    }
    Err(ToolError::new("Project state block is not closed."))
}

fn validate_project_state(state: &Value) -> ToolResult<()> {
    let obj = state
        .as_object()
        .ok_or_else(|| ToolError::new("Project state must be a JSON object."))?;
    let version = obj
        .get("version")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| ToolError::new("Project state version is required."))?;
    if version < 1 {
        return Err(ToolError::new("Project state version is required."));
    }
    let tasks = obj
        .get("tasks")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ToolError::new("Project state tasks must be a list."))?;
    let mut task_ids = std::collections::HashSet::new();
    let mut deps_by_id: Vec<(String, Vec<String>)> = Vec::new();
    for task in tasks {
        let t = task
            .as_object()
            .ok_or_else(|| ToolError::new("Project state task is invalid."))?;
        let task_id = normalize_task_id(t.get("id").and_then(|v| v.as_str()).unwrap_or(""))?;
        if !task_ids.insert(task_id.clone()) {
            return Err(ToolError::new(format!(
                "Duplicate project task id: {task_id}"
            )));
        }
        normalize_task_status(t.get("status").and_then(|v| v.as_str()).unwrap_or(""))?;
        clean_text(
            t.get("title").and_then(|v| v.as_str()).unwrap_or(""),
            "Task title",
            500,
        )?;
        clean_text(
            t.get("updatedAt").and_then(|v| v.as_str()).unwrap_or(""),
            "Task updatedAt",
            80,
        )?;
        if let Some(lr) = t.get("linkedResources") {
            if !lr.is_array() {
                return Err(ToolError::new(
                    "Project task linkedResources must be a list.",
                ));
            }
        }
        let depends_on = t.get("dependsOn").cloned().unwrap_or_else(|| json!([]));
        let deps_arr = depends_on.as_array().ok_or_else(|| {
            ToolError::new("Project task dependsOn must be a list of task ids.")
        })?;
        if !deps_arr.iter().all(|d| d.is_string()) {
            return Err(ToolError::new(
                "Project task dependsOn must be a list of task ids.",
            ));
        }
        if let Some(scope) = t.get("scope") {
            let arr = scope.as_array().ok_or_else(|| {
                ToolError::new("Project task scope must be a list of file paths.")
            })?;
            if !arr.iter().all(|s| s.is_string()) {
                return Err(ToolError::new(
                    "Project task scope must be a list of file paths.",
                ));
            }
        }
        if let Some(acc) = t.get("acceptance") {
            if !acc.is_string() {
                return Err(ToolError::new(
                    "Project task acceptance must be a string.",
                ));
            }
        }
        if let Some(pid) = t.get("planId") {
            if !pid.is_null() && !pid.is_string() {
                return Err(ToolError::new(
                    "Project task planId must be a string or null.",
                ));
            }
        }
        let deps: Vec<String> = deps_arr
            .iter()
            .filter_map(|d| d.as_str())
            .map(|d| normalize_task_id(d))
            .collect::<ToolResult<Vec<_>>>()?;
        deps_by_id.push((task_id, deps));
    }
    if deps_by_id.iter().any(|(_, d)| !d.is_empty()) {
        validate_task_dependency_dag(&deps_by_id)?;
    }
    let notes = obj
        .get("notes")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ToolError::new("Project state notes must be a list."))?;
    for note in notes {
        let n = note
            .as_object()
            .ok_or_else(|| ToolError::new("Project state note is invalid."))?;
        clean_text(
            n.get("id").and_then(|v| v.as_str()).unwrap_or(""),
            "Note id",
            120,
        )?;
        clean_text(
            n.get("text").and_then(|v| v.as_str()).unwrap_or(""),
            "Note text",
            4000,
        )?;
        clean_text(
            n.get("source").and_then(|v| v.as_str()).unwrap_or(""),
            "Note source",
            120,
        )?;
        clean_text(
            n.get("createdAt").and_then(|v| v.as_str()).unwrap_or(""),
            "Note createdAt",
            80,
        )?;
    }
    Ok(())
}

fn validate_task_dependency_dag(deps_by_id: &[(String, Vec<String>)]) -> ToolResult<()> {
    let known: std::collections::HashSet<&str> =
        deps_by_id.iter().map(|(id, _)| id.as_str()).collect();
    for (task_id, deps) in deps_by_id {
        let mut seen = std::collections::HashSet::new();
        for dep in deps {
            if dep == task_id {
                return Err(ToolError::new(format!(
                    "Task {task_id} dependsOn references itself."
                )));
            }
            if !known.contains(dep.as_str()) {
                return Err(ToolError::new(format!(
                    "Task {task_id} dependsOn references unknown task id {dep}."
                )));
            }
            if !seen.insert(dep.as_str()) {
                return Err(ToolError::new(format!(
                    "Task {task_id} has a duplicate dependsOn entry {dep}."
                )));
            }
        }
    }
    let mut in_degree: std::collections::HashMap<&str, usize> = deps_by_id
        .iter()
        .map(|(id, deps)| (id.as_str(), deps.len()))
        .collect();
    let mut dependents: std::collections::HashMap<&str, Vec<&str>> =
        std::collections::HashMap::new();
    for (task_id, deps) in deps_by_id {
        for dep in deps {
            dependents
                .entry(dep.as_str())
                .or_default()
                .push(task_id.as_str());
        }
    }
    let mut queue: Vec<&str> = in_degree
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(id, _)| *id)
        .collect();
    let mut resolved = 0usize;
    while let Some(node) = queue.pop() {
        resolved += 1;
        if let Some(children) = dependents.get(node) {
            for child in children {
                if let Some(d) = in_degree.get_mut(child) {
                    *d -= 1;
                    if *d == 0 {
                        queue.push(child);
                    }
                }
            }
        }
    }
    if resolved != known.len() {
        return Err(ToolError::new(
            "Project task dependsOn graph has a cycle (it must be acyclic).",
        ));
    }
    Ok(())
}

// ── public project document ─────────────────────────────────────────────────

/// In-memory project (metadata + state + markdown frame).
#[derive(Debug, Clone)]
pub struct ProjectDoc {
    pub metadata: Frontmatter,
    pub state: Value,
    pub markdown: String,
    pub revision: String,
    pub path: PathBuf,
    pub modified_at: String,
    block_range: (usize, usize),
}

fn sha256_text(content: &str) -> String {
    let mut h = Sha256::new();
    h.update(content.as_bytes());
    hex::encode(h.finalize())
}

pub fn read_project_file(path: &Path) -> ToolResult<ProjectDoc> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        ToolError::new(format!("Could not read project file: {e}"))
    })?;
    let (metadata, _frontmatter_end) = parse_frontmatter(&content, path)?;
    let (state, block_range) = find_state_block(&content)?;
    let modified = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| {
            let dt: chrono::DateTime<chrono::Utc> = t.into();
            Some(dt.to_rfc3339())
        })
        .unwrap_or_else(now_rfc3339);
    Ok(ProjectDoc {
        metadata,
        state,
        revision: sha256_text(&content),
        path: path.to_path_buf(),
        modified_at: modified,
        markdown: content,
        block_range,
    })
}

pub fn write_project_file(projects_dir: &Path, project: ProjectDoc) -> ToolResult<ProjectDoc> {
    // Re-check confinement on every write (defense in depth vs. stale/mutated path).
    let path = ensure_inside_projects(projects_dir, &project.path)?;
    let (start, end) = project.block_range;
    let state_json = serde_json::to_string_pretty(&project.state).map_err(|e| {
        ToolError::new(format!("Could not serialize project state: {e}"))
    })?;
    let block = format!("{BLOCK_MARKER}\n{state_json}\n{BLOCK_CLOSE}\n");
    let mut content = String::new();
    content.push_str(&project.markdown[..start]);
    content.push_str(&block);
    content.push_str(&project.markdown[end..]);
    // Replace frontmatter while preserving unknown keys.
    let body_after_fm = {
        // Re-parse to get current frontmatter_end of the mutated content string.
        let (_, fm_end) = parse_frontmatter(&content, &path)?;
        content[fm_end..].to_string()
    };
    let content = format!("{}{}", render_frontmatter(&project.metadata), body_after_fm);
    write_text_crash_safe(&path, &content, "project file")?;
    // Re-read is source of truth for block ranges / revision / frontmatter.
    read_project_file(&path)
}

pub fn load_project_locked(projects_dir: &Path, project_id: &str) -> ToolResult<ProjectDoc> {
    let path = project_path(projects_dir, project_id)?;
    if !path.exists() {
        return Err(ToolError::new("Project not found."));
    }
    let lock = project_lock_path(projects_dir, project_id)?;
    with_file_lock(&lock, || {
        if !path.exists() {
            return Err(ToolError::new("Project not found."));
        }
        read_project_file(&path)
    })
}

pub fn task_counts(tasks: &[Value]) -> Map<String, Value> {
    let mut counts = Map::new();
    counts.insert("todo".into(), json!(0));
    counts.insert("wip".into(), json!(0));
    counts.insert("review".into(), json!(0));
    counts.insert("blocked".into(), json!(0));
    counts.insert("done".into(), json!(0));
    counts.insert("total".into(), json!(tasks.len()));
    for task in tasks {
        if let Some(status) = task.get("status").and_then(|v| v.as_str()) {
            if let Some(v) = counts.get_mut(status) {
                if let Some(n) = v.as_u64() {
                    *v = json!(n + 1);
                }
            }
        }
    }
    counts
}

pub fn summarize_project(project: &ProjectDoc) -> Value {
    let tasks = project
        .state
        .get("tasks")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    json!({
        "id": project.metadata.id(),
        "title": project.metadata.title(),
        "status": project.metadata.status(),
        "updatedAt": project.metadata.updated_at(),
        "rootPath": project.metadata.root_path(),
        "revision": project.revision,
        "path": project.path.to_string_lossy(),
        "taskCounts": task_counts(&tasks),
    })
}

/// Public project payload: strip keys starting with `_`.
pub fn public_project(project: &ProjectDoc) -> Value {
    let tasks = project.state.get("tasks").cloned().unwrap_or(json!([]));
    let notes = project.state.get("notes").cloned().unwrap_or(json!([]));
    let mut meta = json!({
        "id": project.metadata.id(),
        "title": project.metadata.title(),
        "status": project.metadata.status(),
        "updatedAt": project.metadata.updated_at(),
        "rootPath": project.metadata.root_path(),
        "censorTrusted": project.metadata.censor_trusted(),
    });
    // Include remaining known frontmatter-ish fields agents may need.
    if let Some(obj) = meta.as_object_mut() {
        for (k, v) in &project.metadata.fields {
            if matches!(
                k.as_str(),
                "id" | "title"
                    | "status"
                    | "updated_at"
                    | "updatedAt"
                    | "root_path"
                    | "rootPath"
                    | "root"
                    | "censor_trusted"
                    | "censorTrusted"
            ) {
                continue;
            }
            // camelCase unknown keys as-is for agents.
            obj.entry(k.clone()).or_insert_with(|| json!(v));
        }
    }
    json!({
        "metadata": meta,
        "state": {
            "version": project.state.get("version").cloned().unwrap_or(json!(1)),
            "tasks": tasks,
            "notes": notes,
        },
        "markdown": project.markdown,
        "revision": project.revision,
        "path": project.path.to_string_lossy(),
        "modifiedAt": project.modified_at,
    })
}

/// Build a minimal project markdown fixture for tests.
pub fn write_test_project(
    projects_dir: &Path,
    project_id: &str,
    title: &str,
    status: &str,
    tasks: Value,
    extra_frontmatter: &[(&str, &str)],
) -> ToolResult<PathBuf> {
    let path = project_path(projects_dir, project_id)?;
    let mut fm = String::from("---\n");
    fm.push_str(&format!("id: {project_id}\n"));
    fm.push_str(&format!("title: {title}\n"));
    fm.push_str(&format!("status: {status}\n"));
    fm.push_str(&format!("updated_at: {}\n", now_rfc3339()));
    for (k, v) in extra_frontmatter {
        fm.push_str(&format!("{k}: {v}\n"));
    }
    fm.push_str("---\n\n");
    let state = json!({
        "version": 1,
        "tasks": tasks,
        "notes": [],
    });
    let body = format!(
        "{fm}# {title}\n\n{BLOCK_MARKER}\n{}\n{BLOCK_CLOSE}\n",
        serde_json::to_string_pretty(&state).unwrap()
    );
    std::fs::write(&path, body).map_err(|e| {
        ToolError::new(format!("Could not write test project: {e}"))
    })?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn project_id_rejects_path_escape() {
        assert!(normalize_project_id("../etc").is_err());
        assert!(normalize_project_id("a/b").is_err());
        assert!(normalize_project_id("ok-project").is_ok());
    }

    #[test]
    fn ensure_inside_rejects_shared_string_prefix_sibling() {
        let tmp = TempDir::new().unwrap();
        let projects = tmp.path().join("projects");
        let projects_backup = tmp.path().join("projects_backup");
        std::fs::create_dir_all(&projects).unwrap();
        std::fs::create_dir_all(&projects_backup).unwrap();
        let backup_file = projects_backup.join("legit.md");
        std::fs::write(&backup_file, "# outside\n").unwrap();

        // String-prefix check would wrongly accept projects_backup under projects.
        let err = ensure_inside_projects(&projects, &backup_file).unwrap_err();
        assert!(
            err.message.contains("escapes"),
            "expected escape error, got: {}",
            err.message
        );

        // Legitimate child is accepted.
        let ok_path = projects.join("ok.md");
        std::fs::write(&ok_path, "# ok\n").unwrap();
        let resolved = ensure_inside_projects(&projects, &ok_path).unwrap();
        assert!(resolved.starts_with(projects.canonicalize().unwrap()));
    }

    #[cfg(unix)]
    #[test]
    fn ensure_inside_rejects_symlink_escape() {
        let tmp = TempDir::new().unwrap();
        let projects = tmp.path().join("projects");
        std::fs::create_dir_all(&projects).unwrap();
        let outside = tmp.path().join("outside.md");
        std::fs::write(&outside, "secret payload\n").unwrap();

        let link = projects.join("legit.md");
        std::os::unix::fs::symlink(&outside, &link).unwrap();

        let err = ensure_inside_projects(&projects, &link).unwrap_err();
        assert!(
            err.message.contains("escapes"),
            "symlink out of projects must be rejected, got: {}",
            err.message
        );

        // write_project_file must also refuse an escaped path (confinement first).
        let doc = ProjectDoc {
            metadata: Frontmatter {
                fields: vec![
                    ("id".into(), "legit".into()),
                    ("title".into(), "Legit".into()),
                    ("status".into(), "active".into()),
                    ("updated_at".into(), now_rfc3339()),
                ],
            },
            state: json!({"version": 1, "tasks": [], "notes": []}),
            markdown: String::new(),
            revision: "x".into(),
            path: link,
            modified_at: now_rfc3339(),
            block_range: (0, 0),
        };
        let write_err = write_project_file(&projects, doc).unwrap_err();
        assert!(
            write_err.message.contains("escapes"),
            "write must refuse escaped path, got: {}",
            write_err.message
        );
        // Outside file must remain untouched by any partial write.
        let outside_text = std::fs::read_to_string(&outside).unwrap();
        assert_eq!(outside_text, "secret payload\n");
    }

    #[test]
    fn round_trip_preserves_censor_trusted_and_unknown() {
        let tmp = TempDir::new().unwrap();
        let projects = tmp.path().join("projects");
        std::fs::create_dir_all(&projects).unwrap();
        let path = write_test_project(
            &projects,
            "demo-proj",
            "Demo",
            "active",
            json!([{
                "id": "T1",
                "title": "One",
                "status": "todo",
                "updatedAt": "2026-01-01T00:00:00Z",
            }]),
            &[("censor_trusted", "true"), ("net_enabled", "true")],
        )
        .unwrap();
        let mut doc = read_project_file(&path).unwrap();
        assert!(doc.metadata.censor_trusted());
        doc.metadata.set("title", "Renamed".into());
        let _ = write_project_file(&projects, doc).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("censor_trusted: true"), "{text}");
        assert!(text.contains("net_enabled: true"), "{text}");
        assert!(text.contains("title: Renamed"), "{text}");
    }
}
