//! Censor ledger tools (P6): `censor_findings`, `censor_dispose`.
//!
//! # Security
//!
//! * Role allowlists + session token via `require_agent_tool`.
//! * Work root path confinement (`validate_project_work_root`).
//! * Relative path validation (no `..` / absolute / `-`-leading).
//! * Strict allowlist on finding fields returned to agents.
//! * Secret redaction on title/body egress.
//! * Verifier adjudication cannot be overridden by a coder (fp/wontfix).
//! * Provenance capped + deduped (shard bloat guard).
//! * Shard lock co-owned with Tauri (`<shard>.json.lock`).

use crate::project_file::normalize_project_id;
use crate::state::{
    now_rfc3339, with_file_lock, write_text_crash_safe, ToolError, ToolResult,
};
use crate::tools::agent_lifecycle::require_agent_tool;
use crate::tools::oracle::{audit_agent_read, resolve_project_work_root};
use crate::tools::mini_coder::validate_mini_rel_path;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const CENSOR_DIR: &str = ".aspis-censor";
const CENSOR_PROVENANCE_MAX: usize = 50;

const CENSOR_SAFE_FINDING_FIELDS: &[&str] = &[
    "id",
    "file",
    "line",
    "severity",
    "category",
    "source",
    "title",
    "body",
    "verdict",
    "disposition",
    "provenance",
];

const CENSOR_DISPOSITION_ACTION: &[(&str, &str)] = &[
    ("open", "reopen"),
    ("fixed", "fixed"),
    ("fp", "fp"),
    ("wontfix", "wontfix"),
];

const CENSOR_VERIFIER_ADJUDICATED: &[&str] = &["fp", "wontfix"];

fn disposition_action(disposition: &str) -> Option<&'static str> {
    CENSOR_DISPOSITION_ACTION
        .iter()
        .find(|(d, _)| *d == disposition)
        .map(|(_, a)| *a)
}

fn validate_censor_rel_path(rel: &str) -> ToolResult<String> {
    // Reuse mini path guard (same contract as Python validate_censor_rel_path).
    validate_mini_rel_path(rel)
}

fn normalize_censor_rel_path(file_rel_path: &str) -> String {
    let mut collapsed = file_rel_path.replace('\\', "/");
    while collapsed.contains("//") {
        collapsed = collapsed.replace("//", "/");
    }
    collapsed
}

fn censor_dir(root: &Path) -> PathBuf {
    root.join(CENSOR_DIR)
}

fn censor_shard_path(root: &Path, file_rel_path: &str) -> ToolResult<PathBuf> {
    validate_censor_rel_path(file_rel_path)?;
    let normalized = normalize_censor_rel_path(file_rel_path);
    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    let name = hex::encode(hasher.finalize());
    Ok(censor_dir(root).join(format!("{name}.json")))
}

fn censor_shard_lock_path(shard: &Path) -> PathBuf {
    let mut s = shard.as_os_str().to_os_string();
    s.push(".lock");
    PathBuf::from(s)
}

/// Second-layer secret redaction (mirrors Python `_redact_secrets` / Rust runners).
pub fn redact_secrets(text: &str) -> String {
    const REDACTED: &str = "[redacted]";
    fn is_token_char(c: char) -> bool {
        c.is_ascii_alphanumeric() || "+/=_-.".contains(c)
    }
    fn looks_secret(tok: &str) -> bool {
        if tok.len() < 12 {
            return false;
        }
        if (tok.starts_with("AKIA") || tok.starts_with("ASIA"))
            && tok.len() == 20
            && tok[4..]
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        {
            return true;
        }
        let has_digit = tok.chars().any(|c| c.is_ascii_digit());
        let has_upper = tok.chars().any(|c| c.is_ascii_uppercase());
        let has_lower = tok.chars().any(|c| c.is_ascii_lowercase());
        let has_symbol = tok.chars().any(|c| "+/=_-.".contains(c));
        // Long pure-uppercase (optional digits) runs look like API-key material
        // (e.g. 16+ char base32 / product keys) — redact even without mixed case.
        if has_upper && !has_lower && !has_symbol && tok.len() >= 16 {
            return true;
        }
        let mostly_separators = has_symbol && !has_digit && !(has_upper && has_lower);
        if mostly_separators {
            return false;
        }
        (has_digit && (has_upper || has_lower)) || (has_upper && has_lower) || has_symbol
    }
    let mut out = String::new();
    let mut token = String::new();
    let flush = |token: &mut String, out: &mut String| {
        if token.is_empty() {
            return;
        }
        if looks_secret(token) {
            out.push_str(REDACTED);
        } else {
            out.push_str(token);
        }
        token.clear();
    };
    for c in text.chars() {
        if is_token_char(c) {
            token.push(c);
        } else {
            flush(&mut token, &mut out);
            out.push(c);
        }
    }
    flush(&mut token, &mut out);
    out
}

fn safe_censor_finding(finding: &Value) -> Value {
    let mut safe = Map::new();
    for key in CENSOR_SAFE_FINDING_FIELDS {
        if *key == "provenance" {
            let entries = finding
                .get("provenance")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|entry| {
                            let e = entry.as_object()?;
                            Some(json!({
                                "actor": e.get("actor").and_then(|v| v.as_str()).unwrap_or(""),
                                "action": e.get("action").and_then(|v| v.as_str()).unwrap_or(""),
                                "role": e.get("role").and_then(|v| v.as_str()).unwrap_or(""),
                                "at": e.get("at").and_then(|v| v.as_str()).unwrap_or(""),
                            }))
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            safe.insert("provenance".into(), json!(entries));
        } else if let Some(value) = finding.get(*key) {
            if matches!(*key, "title" | "body") {
                if let Some(s) = value.as_str() {
                    safe.insert((*key).into(), json!(redact_secrets(s)));
                    continue;
                }
            }
            safe.insert((*key).into(), value.clone());
        }
    }
    Value::Object(safe)
}

fn read_censor_shard(path: &Path) -> Option<Value> {
    let content = fs::read_to_string(path).ok()?;
    let data: Value = serde_json::from_str(&content).ok()?;
    if data.is_object() {
        Some(data)
    } else {
        None
    }
}

fn read_censor_shard_strict(path: &Path) -> ToolResult<Option<Value>> {
    match fs::read_to_string(path) {
        Ok(content) => {
            let data: Value = serde_json::from_str(&content).map_err(|_| {
                ToolError::new(format!(
                    "Corrupt Censor shard (unparseable JSON): {}",
                    path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("shard")
                ))
            })?;
            if !data.is_object() {
                return Err(ToolError::new(format!(
                    "Corrupt Censor shard (not an object): {}",
                    path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("shard")
                )));
            }
            Ok(Some(data))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ToolError::new(format!(
            "Could not read Censor shard: {}: {e}",
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("shard")
        ))),
    }
}

fn read_censor_open_findings(
    root: &Path,
    file_rel_path: Option<&str>,
) -> ToolResult<Vec<Value>> {
    let mut shards: Vec<Value> = Vec::new();
    if let Some(rel) = file_rel_path {
        let shard_path = censor_shard_path(root, rel)?;
        let lock = censor_shard_lock_path(&shard_path);
        let data = with_file_lock(&lock, || Ok::<_, ToolError>(read_censor_shard(&shard_path)))?;
        if let Some(d) = data {
            shards.push(d);
        }
    } else {
        let directory = censor_dir(root);
        let entries = match fs::read_dir(&directory) {
            Ok(e) => {
                let mut v: Vec<_> = e.filter_map(|e| e.ok()).collect();
                v.sort_by_key(|e| e.file_name());
                v
            }
            Err(_) => return Ok(vec![]),
        };
        for entry in entries {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") || !path.is_file() {
                continue;
            }
            let lock = censor_shard_lock_path(&path);
            let data = with_file_lock(&lock, || Ok::<_, ToolError>(read_censor_shard(&path)))?;
            if let Some(d) = data {
                shards.push(d);
            }
        }
    }
    let mut open_findings = Vec::new();
    for shard in shards {
        let Some(findings) = shard.get("findings").and_then(|v| v.as_array()) else {
            continue;
        };
        for finding in findings {
            if !finding.is_object() {
                continue;
            }
            let disposition = finding
                .get("disposition")
                .and_then(|v| v.as_str())
                .unwrap_or("open");
            let disposition = if disposition.is_empty() {
                "open"
            } else {
                disposition
            };
            if disposition == "open" {
                open_findings.push(safe_censor_finding(finding));
            }
        }
    }
    Ok(open_findings)
}

fn drain_censor_queue(root: &Path) -> Vec<Value> {
    let queue_dir = root.join(".aspis").join("censor_queue").join("pending");
    if !queue_dir.is_dir() {
        return vec![];
    }
    let entries = match fs::read_dir(&queue_dir) {
        Ok(e) => {
            let mut v: Vec<_> = e.filter_map(|e| e.ok()).collect();
            v.sort_by_key(|e| e.file_name());
            v
        }
        Err(_) => return vec![],
    };
    let mut findings = Vec::new();
    let mut seen = HashSet::new();
    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") || !path.is_file() {
            continue;
        }
        let data: Value = match fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
        {
            Some(d) => d,
            None => {
                let _ = fs::remove_file(&path);
                continue;
            }
        };
        if let Some(batch) = data.get("findings").and_then(|v| v.as_array()) {
            for f in batch {
                if !f.is_object() {
                    continue;
                }
                let disp = f
                    .get("disposition")
                    .and_then(|v| v.as_str())
                    .unwrap_or("open")
                    .to_ascii_lowercase();
                if disp != "open" {
                    continue;
                }
                let fid = f.get("id").and_then(|v| v.as_str()).unwrap_or("");
                if !fid.is_empty() && seen.insert(fid.to_string()) {
                    findings.push(safe_censor_finding(f));
                }
            }
        }
        let _ = fs::remove_file(&path);
    }
    findings
}

fn last_provenance_role(provenance: &[Value]) -> Option<String> {
    for entry in provenance.iter().rev() {
        if let Some(role) = entry.get("role").and_then(|v| v.as_str()) {
            let role = role.trim();
            if !role.is_empty() {
                return Some(role.to_string());
            }
            return None;
        }
    }
    None
}

fn append_provenance(provenance: &mut Vec<Value>, entry: Value) {
    if let Some(last) = provenance.last() {
        if last.get("actor") == entry.get("actor") && last.get("action") == entry.get("action") {
            return;
        }
    }
    provenance.push(entry);
    if provenance.len() > CENSOR_PROVENANCE_MAX {
        let drop_n = provenance.len() - CENSOR_PROVENANCE_MAX;
        provenance.drain(0..drop_n);
    }
}

fn dispose_censor_finding(
    root: &Path,
    file_rel_path: &str,
    finding_id: &str,
    disposition: &str,
    actor: &str,
    stamp: &str,
    role: &str,
) -> ToolResult<Value> {
    let action = disposition_action(disposition)
        .ok_or_else(|| ToolError::new(format!("Unknown disposition: {disposition}")))?;
    let shard_path = censor_shard_path(root, file_rel_path)?;
    let lock = censor_shard_lock_path(&shard_path);
    with_file_lock(&lock, || {
        let mut shard = read_censor_shard_strict(&shard_path)?
            .ok_or_else(|| ToolError::new(format!("No Censor shard for file: {file_rel_path}")))?;

        let findings_len = shard
            .get("findings")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        if findings_len == 0 {
            return Err(ToolError::new(format!(
                "No Censor finding with id {finding_id} in {file_rel_path}"
            )));
        }
        let target_idx = shard
            .get("findings")
            .and_then(|v| v.as_array())
            .and_then(|arr| {
                arr.iter()
                    .position(|f| f.get("id").and_then(|v| v.as_str()) == Some(finding_id))
            })
            .ok_or_else(|| {
                ToolError::new(format!(
                    "No Censor finding with id {finding_id} in {file_rel_path}"
                ))
            })?;

        let target_snapshot = shard
            .get("findings")
            .and_then(|v| v.as_array())
            .and_then(|a| a.get(target_idx))
            .cloned()
            .ok_or_else(|| {
                ToolError::new(format!(
                    "No Censor finding with id {finding_id} in {file_rel_path}"
                ))
            })?;

        let mut provenance: Vec<Value> = target_snapshot
            .get("provenance")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let mut caller_role = role.trim().to_ascii_lowercase();
        if matches!(caller_role.as_str(), "architect" | "code") {
            caller_role = "coder".into();
        }
        if !matches!(
            caller_role.as_str(),
            "coder" | "verifier" | "mini" | "orchestrator"
        ) {
            caller_role.clear();
        }

        // WARNING 2: coder cannot override verifier adjudication.
        if caller_role == "coder" {
            let current = target_snapshot
                .get("disposition")
                .and_then(|v| v.as_str())
                .unwrap_or("open");
            let current = if current.is_empty() { "open" } else { current };
            if CENSOR_VERIFIER_ADJUDICATED.contains(&current)
                && last_provenance_role(&provenance).as_deref() == Some("verifier")
            {
                return Err(ToolError::new(
                    "A coder cannot override a verifier-adjudicated Censor finding; \
                     ask a verifier to change it.",
                ));
            }
        }

        append_provenance(
            &mut provenance,
            json!({
                "actor": actor,
                "action": action,
                "role": caller_role,
                "at": stamp,
            }),
        );

        {
            let findings = shard
                .get_mut("findings")
                .and_then(|v| v.as_array_mut())
                .ok_or_else(|| {
                    ToolError::new(format!(
                        "No Censor finding with id {finding_id} in {file_rel_path}"
                    ))
                })?;
            if let Some(obj) = findings[target_idx].as_object_mut() {
                obj.insert("disposition".into(), json!(disposition));
                obj.insert("provenance".into(), json!(provenance));
            }
        }
        if let Some(obj) = shard.as_object_mut() {
            obj.insert("updatedAt".into(), json!(stamp));
        }
        let pretty = serde_json::to_string_pretty(&shard)
            .map_err(|e| ToolError::new(format!("Censor shard serialize: {e}")))?;
        write_text_crash_safe(&shard_path, &pretty, "Censor shard")?;
        let safe = shard
            .get("findings")
            .and_then(|v| v.as_array())
            .and_then(|a| a.get(target_idx))
            .map(safe_censor_finding)
            .unwrap_or_else(|| json!({}));
        Ok(safe)
    })
}

// ── public tools ────────────────────────────────────────────────────────────

pub fn censor_findings(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    session_token: Option<&str>,
    project_id: &str,
    file: Option<&str>,
    drain_queue: bool,
) -> ToolResult<Value> {
    let (agent_id, role) =
        require_agent_tool(projects_dir, agent_id, role, "censor_findings", session_token)?;
    let project_id = normalize_project_id(project_id)?;
    let file_arg = file
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| validate_censor_rel_path(s))
        .transpose()?;
    let work_root = resolve_project_work_root(projects_dir, &project_id)?;
    let mut findings = read_censor_open_findings(&work_root, file_arg.as_deref())?;
    if drain_queue {
        let queue_findings = drain_censor_queue(&work_root);
        let mut existing: HashSet<String> = findings
            .iter()
            .filter_map(|f| f.get("id").and_then(|v| v.as_str()).map(str::to_string))
            .collect();
        for qf in queue_findings {
            let id = qf
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if !id.is_empty() && existing.insert(id) {
                findings.push(qf);
            }
        }
    }
    let msg = format!(
        "Read {} open Censor finding(s){}{}",
        findings.len(),
        file_arg
            .as_ref()
            .map(|f| format!(" for {f}"))
            .unwrap_or_default(),
        if drain_queue { " (queue drained)" } else { "" }
    );
    let _ = audit_agent_read(
        projects_dir,
        &agent_id,
        &role,
        "censor_findings",
        &msg,
        Some(&project_id),
    );
    Ok(json!({
        "projectId": project_id,
        "file": file_arg,
        "findings": findings,
        "drainedQueue": drain_queue,
    }))
}

pub fn censor_dispose(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    session_token: Option<&str>,
    project_id: &str,
    file: &str,
    finding_id: &str,
    disposition: &str,
) -> ToolResult<Value> {
    let (agent_id, role) =
        require_agent_tool(projects_dir, agent_id, role, "censor_dispose", session_token)?;
    let project_id = normalize_project_id(project_id)?;
    let file_arg = validate_censor_rel_path(file)?;
    let finding_id = finding_id.trim();
    if finding_id.is_empty() {
        return Err(ToolError::new("Finding id is required."));
    }
    let disposition = disposition.trim();
    if disposition_action(disposition).is_none() {
        return Err(ToolError::new(format!("Unknown disposition: {disposition}")));
    }
    let work_root = resolve_project_work_root(projects_dir, &project_id)?;
    let stamp = now_rfc3339();
    let finding = dispose_censor_finding(
        &work_root,
        &file_arg,
        finding_id,
        disposition,
        &agent_id,
        &stamp,
        &role,
    )?;
    let _ = audit_agent_read(
        projects_dir,
        &agent_id,
        &role,
        "censor_dispose",
        &format!("Disposed Censor finding {finding_id} as {disposition}."),
        Some(&project_id),
    );
    Ok(json!({
        "projectId": project_id,
        "file": file_arg,
        "finding": finding,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{seed_launch_pending};
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

    fn write_proj_with_root(projects: &Path, id: &str, root: &Path) {
        let path = crate::project_file::project_path(projects, id).unwrap();
        let content = format!(
            "---\nid: {id}\ntitle: T\nstatus: active\nroot_path: {}\nupdated_at: 2026-01-01T00:00:00Z\n---\n\n```aspis-project\n{{\"version\":1,\"tasks\":[],\"notes\":[]}}\n```\n",
            root.display()
        );
        fs::write(&path, content).unwrap();
    }

    #[test]
    fn path_escape_rejected() {
        assert!(validate_censor_rel_path("../etc/passwd").is_err());
        assert!(validate_censor_rel_path("/etc/passwd").is_err());
        assert!(validate_censor_rel_path("-rf").is_err());
        assert!(validate_censor_rel_path("src/main.rs").is_ok());
    }

    #[test]
    fn redact_secrets_masks_aws_key() {
        let s = redact_secrets("key=AKIAIOSFODNN7EXAMPLE rest");
        assert!(s.contains("[redacted]"), "{s}");
        assert!(!s.contains("AKIAIOSFODNN7EXAMPLE"), "{s}");
    }

    #[test]
    fn redact_secrets_masks_long_pure_uppercase() {
        // 16+ pure uppercase → secret-shaped; short uppercase prose kept.
        let s = redact_secrets("token=ABCDEFGHIJKLMNOP rest AUTHENTICATION ok");
        assert!(s.contains("[redacted]"), "{s}");
        assert!(!s.contains("ABCDEFGHIJKLMNOP"), "{s}");
        assert!(s.contains("AUTHENTICATION"), "{s}");
    }

    #[test]
    fn safe_finding_allowlist_strips_unknown() {
        let f = json!({
            "id": "F1",
            "file": "a.rs",
            "title": "t",
            "body": "b",
            "disposition": "open",
            "secretField": "LEAK",
            "rawToolOutput": "nope",
        });
        let safe = safe_censor_finding(&f);
        assert!(safe.get("secretField").is_none());
        assert!(safe.get("rawToolOutput").is_none());
        assert_eq!(safe["id"], "F1");
    }

    #[test]
    fn dispose_blocks_coder_override_of_verifier() {
        let _g = env_lock();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("code");
        fs::create_dir_all(censor_dir(&root)).unwrap();
        let file = "src/a.rs";
        let shard = censor_shard_path(&root, file).unwrap();
        let finding = json!({
            "id": "F-1",
            "file": file,
            "title": "x",
            "body": "y",
            "disposition": "fp",
            "provenance": [{
                "actor": "ver-1",
                "action": "fp",
                "role": "verifier",
                "at": "2026-01-01T00:00:00Z"
            }]
        });
        let shard_body = json!({
            "findings": [finding],
            "updatedAt": "2026-01-01T00:00:00Z"
        });
        fs::write(&shard, serde_json::to_string_pretty(&shard_body).unwrap()).unwrap();

        let err = dispose_censor_finding(
            &root,
            file,
            "F-1",
            "open",
            "coder-1",
            "2026-01-02T00:00:00Z",
            "coder",
        )
        .unwrap_err();
        assert!(err.message.contains("cannot override"), "{}", err.message);

        // Verifier may override
        let ok = dispose_censor_finding(
            &root,
            file,
            "F-1",
            "open",
            "ver-2",
            "2026-01-02T00:00:00Z",
            "verifier",
        )
        .unwrap();
        assert_eq!(ok["disposition"], "open");
    }

    #[test]
    fn findings_empty_without_ledger() {
        let _g = env_lock();
        let (_tmp, projects) = temp_projects();
        let ws = projects.parent().unwrap().join("cws");
        let root = ws.join("code");
        fs::create_dir_all(&root).unwrap();
        std::env::set_var("ASPIS_WORKSPACE_ROOT", ws.to_str().unwrap());
        write_proj_with_root(&projects, "cen-a", &root);
        let tok = register(&projects, "coder-cen", "coder");
        let out = censor_findings(
            &projects,
            "coder-cen",
            "coder",
            Some(&tok),
            "cen-a",
            None,
            false,
        )
        .unwrap();
        assert_eq!(out["findings"].as_array().unwrap().len(), 0);
        std::env::remove_var("ASPIS_WORKSPACE_ROOT");
    }

    #[test]
    fn orchestrator_cannot_use_censor() {
        let _g = env_lock();
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "orch-cen", "orchestrator");
        let err = censor_findings(
            &projects,
            "orch-cen",
            "orchestrator",
            Some(&tok),
            "any-proj",
            None,
            false,
        )
        .unwrap_err();
        assert!(err.message.contains("cannot use"), "{}", err.message);
    }
}
