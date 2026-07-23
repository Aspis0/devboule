use super::model::{ProjectMetadata, ProjectStateBlock};
use super::projects::{
    clean_milestone_date, clean_required, normalize_project_id, normalize_project_root,
    normalize_project_status, normalize_task_category, normalize_task_status, now,
    project_file_lock, project_file_lock_spin, project_lock_path, read_project_file,
    validate_task_id, ParsedProject, ProjectFileLock,
};
const BLOCK_MARKER: &str = "```aspis-project";
const BLOCK_CLOSE: &str = "```";
use super::fs_replace::replace_file_with_backup;
use chrono::Utc;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::time::Duration;

const PROJECT_LOCK_BRIEF_ATTEMPTS: u32 = 1;

pub(crate) fn read_project_file_locked(path: &Path) -> Result<ParsedProject, String> {
    let _file_guard = project_file_lock(&project_lock_path(path))?;
    if !path.exists() {
        return Err("Project not found.".into());
    }
    read_project_file(path)
}

/// FAIL-OPEN, NON-BLOCKING project read for the 5s Polis refresh path ONLY.
/// Identical to [`read_project_file_locked`] except it uses the single-try
/// [`PROJECT_LOCK_BRIEF_ATTEMPTS`] budget (one immediate `try_lock`, no sleep) and,
/// on lock CONTENTION, returns `Ok(None)` so the caller can SKIP this project file
/// for the current cycle instead of parking a Tauri worker thread for up to 5s
/// behind a writer. Skipping is safe here: the suspect overlay is non-critical and
/// the previous overlay state survives one tick — the next 5s refresh retries it.
///
/// `Ok(None)` is ALSO returned when the file is MISSING (FIX 5): a project `.md`
/// deleted between the dir listing and the lock is a benign "nothing to read this
/// cycle", NOT a fault — the old `Err("Project not found.")` violated this fn's
/// contract that `Err` is reserved for a genuine open/parse failure. `Err` still
/// surfaces a real IO/parse fault (fail-open at the caller, but distinguished so we
/// never mask it as "contended"). This MUST NOT be used by any read-modify-write or
/// correctness-critical reader.
pub(crate) fn try_read_project_file_locked_briefly(path: &Path) -> Result<Option<ParsedProject>, String> {
    let Some(_file_guard) =
        project_file_lock_spin(&project_lock_path(path), PROJECT_LOCK_BRIEF_ATTEMPTS)?
    else {
        // Contended right now — skip for this cycle.
        return Ok(None);
    };
    if !path.exists() {
        // Deleted between dir listing and lock — a benign skip, not an Err (FIX 5).
        return Ok(None);
    }
    read_project_file(path).map(Some)
}

pub(crate) fn parse_frontmatter(content: &str, path: &Path) -> Result<(ProjectMetadata, usize), String> {
    let mut offset = 0usize;
    let mut lines = content.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return Err("Project file is empty.".into());
    };
    offset += first.len();
    if first.trim_end() != "---" {
        return Err(format!(
            "Project file {} is missing frontmatter.",
            path.display()
        ));
    }
    let mut raw = String::new();
    for line in lines {
        offset += line.len();
        if line.trim_end() == "---" {
            let fields = parse_simple_yaml(&raw);
            let fallback_id = path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("project");
            let canonical_id = normalize_project_id(fallback_id)?;
            let id =
                normalize_project_id(fields.get("id").map(String::as_str).unwrap_or(fallback_id))?;
            if path.is_absolute() && path.exists() && id != canonical_id {
                return Err(format!(
                    "Project file {} has id '{}' but filename expects '{}'.",
                    path.display(),
                    id,
                    canonical_id
                ));
            }
            let title = clean_required(
                fields
                    .get("title")
                    .map(String::as_str)
                    .unwrap_or(id.as_str()),
                "Project title",
            )?;
            let status = normalize_project_status(
                fields.get("status").map(String::as_str).unwrap_or("active"),
            )?;
            let updated_at = fields
                .get("updated_at")
                .or_else(|| fields.get("updatedAt"))
                .cloned()
                .unwrap_or_else(now);
            let root_path = fields
                .get("root_path")
                .or_else(|| fields.get("rootPath"))
                .or_else(|| fields.get("root"))
                .and_then(|value| normalize_project_root(Some(value)));
            // BLOCKER B: trust flag. Absent (old files) → false. Only an explicit
            // "true" (case-insensitive) trusts; any other value stays false
            // (fail-closed). NO-CHURN: the serializer omits this key when false.
            let censor_trusted = fields
                .get("censor_trusted")
                .or_else(|| fields.get("censorTrusted"))
                .map(|value| value.trim().eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            // SANDBOX phase 2 (review F1): READ net_enabled from the frontmatter — fail-closed,
            // NO-CHURN, same shape as censor_trusted.
            let net_enabled = fields
                .get("net_enabled")
                .or_else(|| fields.get("netEnabled"))
                .map(|value| value.trim().eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            // SANDBOX broker Slice 1: read sandbox_mode. Missing key → Ask (default, fail-open
            // for prompting). Unknown/unrecognised value → Ask (defensive; never errors the
            // whole parse). NO-CHURN: Ask is not written.
            //
            // Use a direct closed-enum match instead of JSON-string construction to avoid
            // fragility from escaping (e.g. a value containing a backslash would silently
            // produce a serde error and fall through to Ask, hiding the bad on-disk value;
            // a direct match makes the intent explicit and works for any byte sequence).
            let sandbox_mode = fields
                .get("sandbox_mode")
                .or_else(|| fields.get("sandboxMode"))
                .map(|value| match value.trim() {
                    "ask" => crate::backend::broker::SandboxMode::Ask,
                    "autoAcceptInWorkspace" => {
                        crate::backend::broker::SandboxMode::AutoAcceptInWorkspace
                    }
                    "unattended" => crate::backend::broker::SandboxMode::Unattended,
                    // Any unrecognised string (typo, future variant, garbage) → Ask.
                    // This is intentionally tolerant: a bad on-disk value must never prevent
                    // opening the project.
                    _ => crate::backend::broker::SandboxMode::Ask,
                })
                .unwrap_or_default();
            // SANDBOX broker Slice 2: read working_set. The value is a compact JSON array
            // stored on a single frontmatter line, e.g. `working_set: ["/tmp/a","/tmp/b"]`.
            // Missing key → empty (NO-CHURN: a project written before this feature was added
            // has no key and must load with an empty working set). An unparseable value degrades
            // to empty rather than erroring the whole parse (same tolerant posture as sandbox_mode).
            let working_set: Vec<String> = fields
                .get("working_set")
                .or_else(|| fields.get("workingSet"))
                .and_then(|value| serde_json::from_str(value.trim()).ok())
                .unwrap_or_default();
            // Slice 5c: read agent_controls — a compact JSON object on a single frontmatter line,
            // e.g. `agent_controls: {"effort":"high"}`. Missing key → default (NO-CHURN: a project
            // written before this feature has no key). An unparseable value degrades to default
            // (same tolerant posture as working_set), never erroring the whole parse.
            let agent_controls: crate::backend::model::AgentControls = fields
                .get("agent_controls")
                .or_else(|| fields.get("agentControls"))
                .and_then(|value| serde_json::from_str(value.trim()).ok())
                .unwrap_or_default();
            // P6b: read the per-project Main-coder engine override. Missing key → None (NO-CHURN:
            // a project written before this feature has no key and must load with no override).
            // A key present but blank after trim → None (a blank value carries no engine, same
            // tolerant posture as sandbox_mode's blank → default). Stored as a plain unquoted
            // engine id (no colons), so a first-colon split yields the value verbatim.
            let main_coder: Option<String> = fields
                .get("main_coder")
                .or_else(|| fields.get("mainCoder"))
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            return Ok((
                ProjectMetadata {
                    id,
                    title,
                    status,
                    updated_at,
                    root_path,
                    censor_trusted,
                    net_enabled,
                    sandbox_mode,
                    working_set,
                    agent_controls,
                    main_coder,
                },
                offset,
            ));
        }
        raw.push_str(line);
    }
    Err(format!(
        "Project file {} has unterminated frontmatter.",
        path.display()
    ))
}

fn parse_simple_yaml(raw: &str) -> HashMap<String, String> {
    raw.lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            Some((
                key.trim().to_string(),
                unquote_simple_yaml_value(value.trim()),
            ))
        })
        .collect()
}

fn unquote_simple_yaml_value(value: &str) -> String {
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        return value[1..value.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\");
    }
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        return value[1..value.len() - 1].replace("''", "'");
    }
    value.to_string()
}

pub(crate) fn parse_state_block(content: &str) -> Result<(ProjectStateBlock, std::ops::Range<usize>), String> {
    let start = content
        .find(BLOCK_MARKER)
        .ok_or_else(|| "Project file is missing ```aspis-project block.".to_string())?;
    let body_start = content[start..]
        .find('\n')
        .map(|offset| start + offset + 1)
        .ok_or_else(|| "Project state block is malformed.".to_string())?;
    let (body_end, close_end) = find_state_block_close(content, body_start)?;
    let state = serde_json::from_str::<ProjectStateBlock>(content[body_start..body_end].trim())
        .map_err(|e| format!("Project state JSON is invalid: {e}"))?;
    Ok((state, start..close_end))
}

/// The SINGLE read choke point for a parsed project's state. Validates the
/// invariants that MUST hold (status, id shape, uniqueness, non-empty title) and
/// NORMALIZES the `category` in place: a hand-edited `"category": "Bug"` is
/// lowercased to `"bug"` so it passes `collect_open_bug_suspects`'s
/// `== Some("bug")` filter (FIX 1). An INVALID category (e.g. `"banana"` in a
/// legacy/hand-edited file) is the GENTLE-degrade case: it DROPS to `None`
/// (uncategorized) rather than erroring the entire project load — one bad value
/// must never make a whole project unreadable. A degraded card raises no Polis
/// smoke, which is the honest outcome (it is no longer a recognized bug card).
/// Takes `&mut` because normalization writes back onto the loaded tasks.
pub(crate) fn validate_project_state(state: &mut ProjectStateBlock) -> Result<(), String> {
    let mut task_ids = HashSet::new();
    for task in &mut state.tasks {
        normalize_task_status(&task.status)?;
        validate_task_id(&task.id)?;
        if !task_ids.insert(task.id.clone()) {
            return Err(format!("Duplicate project task id: {}", task.id));
        }
        clean_required(&task.title, "Task title")?;
        // Normalize the optional category in place: lowercase a valid value so the
        // bug-only suspect filter recognizes it; degrade an invalid value to None
        // (do NOT error the load — see the doc above).
        task.category = task
            .category
            .as_deref()
            .and_then(|value| normalize_task_category(value).ok());
    }
    // Normalize milestones on load mirroring the task degrade pattern: a hand-
    // edited / externally-written file must not surface bad entries (React key
    // collisions from duplicate ids, stale entries from malformed dates persisted
    // forever). DROP an entry with an empty title or a date that fails the strict
    // `clean_milestone_date` check, and DEDUPE by id keeping the first occurrence.
    // This degrades (drops the bad ones) rather than erroring the whole load — one
    // bad milestone must never make a project unreadable.
    let mut milestone_ids = HashSet::new();
    state.milestones.retain(|milestone| {
        if milestone.title.trim().is_empty() {
            return false;
        }
        if clean_milestone_date(&milestone.date).is_err() {
            return false;
        }
        milestone_ids.insert(milestone.id.clone())
    });
    Ok(())
}

fn find_state_block_close(content: &str, body_start: usize) -> Result<(usize, usize), String> {
    let mut offset = body_start;
    for line in content[body_start..].split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();
        if line.trim() == BLOCK_CLOSE {
            return Ok((line_start, offset));
        }
    }
    Err("Project state block is not closed.".into())
}

pub(crate) fn write_project_file(project: &ParsedProject) -> Result<(), String> {
    let mut content = project.content.clone();
    let block = format!(
        "{BLOCK_MARKER}\n{}\n{BLOCK_CLOSE}\n",
        serde_json::to_string_pretty(&project.state)
            .map_err(|e| format!("Project state could not be serialized: {e}"))?
    );
    content.replace_range(project.block_range.clone(), &block);
    content = replace_frontmatter(&content, &project.metadata)?;
    let suffix = format!(
        "{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );
    let temp_path = project.path.with_extension(format!("md.{suffix}.tmp"));
    let backup_path = project.path.with_extension(format!("md.{suffix}.bak"));
    fs::write(&temp_path, content)
        .map_err(|e| format!("Could not write project temp file: {e}"))?;
    replace_file_with_backup(&temp_path, &project.path, &backup_path, "project file")
}

pub(crate) fn replace_frontmatter(content: &str, metadata: &ProjectMetadata) -> Result<String, String> {
    let (_, end) = parse_frontmatter(content, Path::new("project.md"))?;
    let frontmatter = format!(
        "---\nid: {}\ntitle: {}\nstatus: {}\nupdated_at: {}\n{}{}{}{}{}{}{}---\n",
        metadata.id,
        metadata.title,
        metadata.status,
        metadata.updated_at,
        metadata
            .root_path
            .as_ref()
            .map(|value| format!("root_path: \"{}\"\n", yaml_double_quote_inner(value)))
            .unwrap_or_default(),
        censor_trusted_frontmatter_line(metadata.censor_trusted),
        net_enabled_frontmatter_line(metadata.net_enabled),
        sandbox_mode_frontmatter_line(metadata.sandbox_mode),
        working_set_frontmatter_line(&metadata.working_set),
        agent_controls_frontmatter_line(&metadata.agent_controls),
        main_coder_frontmatter_line(&metadata.main_coder),
    );
    Ok(format!("{frontmatter}{}", &content[end..]))
}

/// BLOCKER B NO-CHURN: emit the `censor_trusted: true` frontmatter line ONLY when
/// trusted. When false (the default for every pre-existing project) we emit
/// nothing, so serializing an untrusted project never injects a new key — the
/// on-disk bytes stay identical and the content hash / git status don't churn.
fn censor_trusted_frontmatter_line(trusted: bool) -> String {
    if trusted {
        "censor_trusted: true\n".to_string()
    } else {
        String::new()
    }
}

/// SANDBOX phase 2 NO-CHURN: emit `net_enabled: true` ONLY when enabled; nothing when false, so a
/// pre-existing project's on-disk bytes stay identical (no content-hash / git churn).
fn net_enabled_frontmatter_line(enabled: bool) -> String {
    if enabled {
        "net_enabled: true\n".to_string()
    } else {
        String::new()
    }
}

/// SANDBOX broker Slice 1 NO-CHURN: emit `sandbox_mode: <variant>` ONLY when the mode differs
/// from the default (`Ask`), so pre-existing project files stay byte-stable.
/// The on-disk value is the serde camelCase string without quotes (e.g. `autoAcceptInWorkspace`).
fn sandbox_mode_frontmatter_line(mode: crate::backend::broker::SandboxMode) -> String {
    if crate::backend::broker::is_default_sandbox_mode(&mode) {
        String::new()
    } else {
        // Serialize via serde to get the canonical camelCase string, then strip the JSON quotes.
        let json = serde_json::to_string(&mode).unwrap_or_default();
        let value = json.trim_matches('"');
        format!("sandbox_mode: {value}\n")
    }
}

/// SANDBOX broker Slice 2 NO-CHURN: emit `working_set: [...]` ONLY when non-empty.
/// The value is a compact JSON array on ONE line so `parse_simple_yaml` can split on the FIRST
/// colon and still get a parseable value (the entries are JSON strings, no unescaped colons).
/// An empty working_set emits nothing — pre-existing project files stay byte-stable.
fn working_set_frontmatter_line(folders: &[String]) -> String {
    if folders.is_empty() {
        return String::new();
    }
    // serde_json compact array: `["/tmp/a","/tmp/b"]` — no spaces, no unescaped colons,
    // stays on one line, parses back cleanly via `serde_json::from_str` in parse_frontmatter.
    let json = serde_json::to_string(folders).unwrap_or_else(|_| "[]".to_string());
    format!("working_set: {json}\n")
}

/// Slice 5c NO-CHURN: emit `agent_controls: {...}` ONLY when at least one control is set.
/// The value is a compact JSON object on ONE line (serde escapes any `:`/`"`/newline inside
/// `systemPrompt`, so it parses back cleanly via `serde_json::from_str` and never breaks the
/// single-line frontmatter). An all-unset AgentControls emits nothing — byte-stable on disk.
fn agent_controls_frontmatter_line(controls: &crate::backend::model::AgentControls) -> String {
    if controls.is_default() {
        return String::new();
    }
    let json = serde_json::to_string(controls).unwrap_or_else(|_| "{}".to_string());
    format!("agent_controls: {json}\n")
}

/// P6b trust-boundary check for a per-project Main-coder engine id before it is written
/// VERBATIM onto a single frontmatter line (`main_coder: <value>`). A control character —
/// above all a newline/CR — would break the single-line invariant and could inject a
/// premature `---`, corrupting the project file or smuggling frontmatter keys. The id comes
/// from a known engine set, but the setter command is a trust boundary, so we reject rather
/// than silently sanitize. A colon is allowed (the parser splits on the FIRST colon only).
pub(crate) fn validate_main_coder_engine_id(value: &str) -> Result<(), String> {
    if value.chars().any(char::is_control) {
        return Err("Invalid Main-coder engine id: control characters are not allowed.".into());
    }
    Ok(())
}

/// P6b NO-CHURN: emit `main_coder: <engine-id>` ONLY when a per-project override is set.
/// `None` (the default for every pre-existing project) emits nothing, so serializing a
/// project without an override never injects the key — the on-disk bytes stay identical.
/// The value is a plain unquoted engine/client id (no colons or newlines by construction —
/// it comes from the configured Main-coder engine set), read back verbatim by the parser.
fn main_coder_frontmatter_line(main_coder: &Option<String>) -> String {
    match main_coder
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        Some(value) => format!("main_coder: {value}\n"),
        None => String::new(),
    }
}

pub(crate) fn initial_project_markdown(
    metadata: &ProjectMetadata,
    state: &ProjectStateBlock,
) -> Result<String, String> {
    Ok(format!(
        "---\nid: {}\ntitle: {}\nstatus: {}\nupdated_at: {}\n{}{}{}{}{}{}{}---\n\n# Obiettivi\n- Definisci qui gli obiettivi operativi del progetto.\n\n{BLOCK_MARKER}\n{}\n{BLOCK_CLOSE}\n\n# Note libere\n",
        metadata.id,
        metadata.title,
        metadata.status,
        metadata.updated_at,
        metadata
            .root_path
            .as_ref()
            .map(|value| format!("root_path: \"{}\"\n", yaml_double_quote_inner(value)))
            .unwrap_or_default(),
        censor_trusted_frontmatter_line(metadata.censor_trusted),
        net_enabled_frontmatter_line(metadata.net_enabled),
        sandbox_mode_frontmatter_line(metadata.sandbox_mode),
        working_set_frontmatter_line(&metadata.working_set),
        agent_controls_frontmatter_line(&metadata.agent_controls),
        main_coder_frontmatter_line(&metadata.main_coder),
        serde_json::to_string_pretty(state)
            .map_err(|e| format!("Project state could not be serialized: {e}"))?
    ))
}

fn yaml_double_quote_inner(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::model::{ProjectMilestone, ProjectNote, ProjectTask};
    use super::super::projects::collect_open_bug_suspects;
    use std::path::PathBuf;

    fn task(status: &str) -> ProjectTask {
        ProjectTask {
            id: format!("T-{status}"),
            title: "Task".into(),
            status: status.into(),
            priority: None,
            assignee: None,
            due: None,
            updated_at: "2026-05-28T00:00:00Z".into(),
            category: None,
            description: None,
            suspect_file_ids: Vec::new(),
            depends_on: Vec::new(),
            scope: Vec::new(),
            acceptance: String::new(),
            plan_id: None,
            weight: String::new(),
        }
    }

    /// Helper: create a valid project markdown file with a single task `T1` into a fresh
    /// temp dir and return its path. The filename must match the frontmatter id.
    fn write_temp_project(slug: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "aspis-pf-{}-{}-{}",
            slug,
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let metadata = ProjectMetadata {
            id: slug.into(),
            title: "Project File Test".into(),
            status: "active".into(),
            updated_at: "2026-05-28T00:00:00Z".into(),
            root_path: None,
            censor_trusted: false,
            net_enabled: false,
            sandbox_mode: crate::backend::broker::SandboxMode::default(),
            working_set: Vec::new(),
            agent_controls: Default::default(),
            main_coder: None,
        };
        let state = ProjectStateBlock {
            version: 1,
            tasks: vec![ProjectTask {
                id: "T1".into(),
                title: "Worker 500 on cold start".into(),
                status: "todo".into(),
                priority: None,
                assignee: None,
                due: None,
                updated_at: "2026-05-28T00:00:00Z".into(),
                category: Some("bug".into()),
                description: None,
                suspect_file_ids: Vec::new(),
                depends_on: Vec::new(),
                scope: Vec::new(),
                acceptance: String::new(),
                plan_id: None,
                weight: String::new(),
            }],
            notes: Vec::new(),
            milestones: Vec::new(),
        };
        let markdown = initial_project_markdown(&metadata, &state).unwrap();
        let path = root.join(format!("{slug}.md"));
        fs::write(&path, markdown).unwrap();
        (root, path)
    }

    #[test]
    fn censor_trusted_frontmatter_no_churn_when_false() {
        // BLOCKER B NO-CHURN: an untrusted (default) project must NOT emit the
        // censor_trusted line, so serializing a pre-existing project is byte-stable.
        assert_eq!(censor_trusted_frontmatter_line(false), "");
        assert_eq!(
            censor_trusted_frontmatter_line(true),
            "censor_trusted: true\n"
        );
    }

    #[test]
    fn censor_trusted_roundtrips_and_old_files_default_false() {
        // An old file with NO censor_trusted line parses as untrusted (back-compat).
        let old = "---\nid: proj-x\ntitle: P\nstatus: active\nupdated_at: t\n---\n";
        let (meta, _) = parse_frontmatter(old, Path::new("proj-x.md")).unwrap();
        assert!(!meta.censor_trusted, "missing key must default to false");

        // Serializing a TRUSTED project emits the line; re-parsing reads it back true.
        let trusted = ProjectMetadata {
            id: "proj-x".into(),
            title: "P".into(),
            status: "active".into(),
            updated_at: "t".into(),
            root_path: None,
            censor_trusted: true,
            net_enabled: false,
            sandbox_mode: crate::backend::broker::SandboxMode::default(),
            working_set: Vec::new(),
            agent_controls: Default::default(),
            main_coder: None,
        };
        let serialized = replace_frontmatter(old, &trusted).unwrap();
        assert!(serialized.contains("censor_trusted: true"));
        let (reparsed, _) = parse_frontmatter(&serialized, Path::new("proj-x.md")).unwrap();
        assert!(reparsed.censor_trusted);

        // Serializing an UNTRUSTED project (default) must NOT inject the key.
        let untrusted = ProjectMetadata {
            censor_trusted: false,
            agent_controls: Default::default(),
            ..trusted
        };
        let serialized_off = replace_frontmatter(old, &untrusted).unwrap();
        assert!(!serialized_off.contains("censor_trusted"));
    }

    #[test]
    fn net_enabled_roundtrips_and_old_files_default_false() {
        // An old file with NO net_enabled line parses as net-DENIED (back-compat / fail-closed).
        let old = "---\nid: proj-n\ntitle: P\nstatus: active\nupdated_at: t\n---\n";
        let (meta, _) = parse_frontmatter(old, Path::new("proj-n.md")).unwrap();
        assert!(!meta.net_enabled, "missing key must default to false");

        // Serializing an UNBLOCKED project emits the line; re-parsing reads it back true.
        let enabled = ProjectMetadata {
            id: "proj-n".into(),
            title: "P".into(),
            status: "active".into(),
            updated_at: "t".into(),
            root_path: None,
            censor_trusted: false,
            net_enabled: true,
            sandbox_mode: crate::backend::broker::SandboxMode::default(),
            working_set: Vec::new(),
            agent_controls: Default::default(),
            main_coder: None,
        };
        let serialized = replace_frontmatter(old, &enabled).unwrap();
        assert!(serialized.contains("net_enabled: true"));
        let (reparsed, _) = parse_frontmatter(&serialized, Path::new("proj-n.md")).unwrap();
        assert!(reparsed.net_enabled, "net_enabled must round-trip true");

        // Serializing a net-DISABLED project (default) must NOT inject the key (NO-CHURN).
        let disabled = ProjectMetadata {
            net_enabled: false,
            agent_controls: Default::default(),
            ..enabled
        };
        let serialized_off = replace_frontmatter(old, &disabled).unwrap();
        assert!(!serialized_off.contains("net_enabled"));
    }

    // ── sandbox_mode NO-CHURN round-trip ──────────────────────────────────────────────

    /// SANDBOX broker Slice 1: `Ask` (the default) is omitted from the frontmatter so
    /// pre-existing project files stay byte-stable (NO-CHURN).  Non-default variants
    /// round-trip through parse→serialize→parse.
    #[test]
    fn sandbox_mode_roundtrips_and_old_files_default_ask() {
        use crate::backend::broker::SandboxMode;

        // An old file with NO sandbox_mode line must parse as Ask (back-compat / fail-open).
        let old = "---\nid: proj-s\ntitle: P\nstatus: active\nupdated_at: t\n---\n";
        let (meta, _) = parse_frontmatter(old, Path::new("proj-s.md")).unwrap();
        assert_eq!(
            meta.sandbox_mode,
            SandboxMode::Ask,
            "missing key must default to Ask"
        );

        // Serializing a project with Ask (default) must NOT inject the key (NO-CHURN).
        let ask_meta = ProjectMetadata {
            id: "proj-s".into(),
            title: "P".into(),
            status: "active".into(),
            updated_at: "t".into(),
            root_path: None,
            censor_trusted: false,
            net_enabled: false,
            sandbox_mode: SandboxMode::Ask,
            working_set: Vec::new(),
            agent_controls: Default::default(),
            main_coder: None,
        };
        let serialized_ask = replace_frontmatter(old, &ask_meta).unwrap();
        assert!(
            !serialized_ask.contains("sandbox_mode"),
            "Ask must not write sandbox_mode key (NO-CHURN)"
        );

        // Serializing AutoAcceptInWorkspace emits the key; re-parsing reads it back.
        let auto_meta = ProjectMetadata {
            sandbox_mode: SandboxMode::AutoAcceptInWorkspace,
            agent_controls: Default::default(),
            ..ask_meta
        };
        let serialized_auto = replace_frontmatter(old, &auto_meta).unwrap();
        assert!(
            serialized_auto.contains("sandbox_mode: autoAcceptInWorkspace"),
            "AutoAcceptInWorkspace must write the key"
        );
        let (reparsed_auto, _) =
            parse_frontmatter(&serialized_auto, Path::new("proj-s.md")).unwrap();
        assert_eq!(
            reparsed_auto.sandbox_mode,
            SandboxMode::AutoAcceptInWorkspace,
            "AutoAcceptInWorkspace must round-trip"
        );

        // Serializing Unattended emits the key; re-parsing reads it back.
        let unattended_meta = ProjectMetadata {
            sandbox_mode: SandboxMode::Unattended,
            agent_controls: Default::default(),
            ..reparsed_auto
        };
        let serialized_unattended =
            replace_frontmatter(&serialized_auto, &unattended_meta).unwrap();
        assert!(
            serialized_unattended.contains("sandbox_mode: unattended"),
            "Unattended must write the key"
        );
        let (reparsed_unattended, _) =
            parse_frontmatter(&serialized_unattended, Path::new("proj-s.md")).unwrap();
        assert_eq!(
            reparsed_unattended.sandbox_mode,
            SandboxMode::Unattended,
            "Unattended must round-trip"
        );
    }

    /// FIX A: a garbage `sandbox_mode` value on disk must never fail the parse.
    /// The project must load successfully and the mode must fall back to `Ask`.
    ///
    /// This test also covers values that would have broken the old serde JSON-string
    /// approach (e.g. a backslash in the value produces invalid JSON but is a valid
    /// ASCII string that our direct match handles correctly by falling through to Ask).
    #[test]
    fn sandbox_mode_garbage_value_defaults_to_ask_and_does_not_error() {
        use crate::backend::broker::SandboxMode;

        // Plain unrecognised token.
        let bogus =
            "---\nid: proj-g\ntitle: G\nstatus: active\nupdated_at: t\nsandbox_mode: bogus\n---\n";
        let (meta, _) = parse_frontmatter(bogus, Path::new("proj-g.md"))
            .expect("parse must succeed even with an unrecognised sandbox_mode value");
        assert_eq!(
            meta.sandbox_mode,
            SandboxMode::Ask,
            "unrecognised value must fall back to Ask"
        );

        // Value with a backslash — would produce invalid JSON in the old code.
        let backslash =
            "---\nid: proj-g\ntitle: G\nstatus: active\nupdated_at: t\nsandbox_mode: bogus\\value\n---\n";
        let (meta_bs, _) = parse_frontmatter(backslash, Path::new("proj-g.md"))
            .expect("parse must succeed even with a backslash in sandbox_mode");
        assert_eq!(
            meta_bs.sandbox_mode,
            SandboxMode::Ask,
            "backslash-containing value must fall back to Ask (not error)"
        );

        // Completely empty value (key present, value is blank after trim).
        let empty =
            "---\nid: proj-g\ntitle: G\nstatus: active\nupdated_at: t\nsandbox_mode:  \n---\n";
        // Note: parse_simple_yaml splits on the FIRST ':', so "sandbox_mode:  " yields
        // key="sandbox_mode", value="  " (whitespace only). After trim → "". Not a known
        // variant → Ask.
        let (meta_e, _) = parse_frontmatter(empty, Path::new("proj-g.md"))
            .expect("parse must succeed even with a blank sandbox_mode value");
        assert_eq!(
            meta_e.sandbox_mode,
            SandboxMode::Ask,
            "blank value must fall back to Ask"
        );
    }

    /// P6b: the per-project Main-coder engine override round-trips, an old file with no key
    /// parses as `None`, and serializing `None` is byte-identical (NO-CHURN).
    #[test]
    fn main_coder_override_roundtrips_and_old_files_have_no_override() {
        // An old file with NO main_coder line must parse as None (back-compat / NO-CHURN).
        let old = "---\nid: proj-mc\ntitle: P\nstatus: active\nupdated_at: t\n---\n";
        let (meta, _) = parse_frontmatter(old, Path::new("proj-mc.md")).unwrap();
        assert_eq!(meta.main_coder, None, "missing key must default to None");

        // Serializing a project with None (default) must NOT inject the key AND must be
        // byte-identical to the original frontmatter (the strongest NO-CHURN guarantee).
        let none_meta = ProjectMetadata {
            id: "proj-mc".into(),
            title: "P".into(),
            status: "active".into(),
            updated_at: "t".into(),
            root_path: None,
            censor_trusted: false,
            net_enabled: false,
            sandbox_mode: crate::backend::broker::SandboxMode::default(),
            working_set: Vec::new(),
            agent_controls: Default::default(),
            main_coder: None,
        };
        let serialized_none = replace_frontmatter(old, &none_meta).unwrap();
        assert!(
            !serialized_none.contains("main_coder"),
            "None must not write the main_coder key (NO-CHURN)"
        );
        assert_eq!(
            serialized_none, old,
            "a None override must serialize byte-identical to the original"
        );

        // Serializing Some("codex") emits the key; re-parsing reads it back verbatim.
        let codex_meta = ProjectMetadata {
            main_coder: Some("codex".into()),
            ..none_meta
        };
        let serialized_codex = replace_frontmatter(old, &codex_meta).unwrap();
        assert!(
            serialized_codex.contains("main_coder: codex"),
            "an override must write the main_coder key"
        );
        let (reparsed, _) = parse_frontmatter(&serialized_codex, Path::new("proj-mc.md")).unwrap();
        assert_eq!(
            reparsed.main_coder.as_deref(),
            Some("codex"),
            "the override must round-trip"
        );

        // A key present but blank after trim → None (tolerant, like sandbox_mode blank → default).
        let blank =
            "---\nid: proj-mc\ntitle: P\nstatus: active\nupdated_at: t\nmain_coder:  \n---\n";
        let (meta_blank, _) = parse_frontmatter(blank, Path::new("proj-mc.md")).unwrap();
        assert_eq!(
            meta_blank.main_coder, None,
            "a blank main_coder value must parse as None"
        );
    }

    /// P6b SECURITY: the engine-id validator must reject any control character (the frontmatter
    /// injection vector) while accepting ordinary engine/client ids.
    #[test]
    fn main_coder_engine_id_rejects_control_characters() {
        // Legit ids pass.
        for ok in ["codex", "claude", "omlx", "my-custom_client.v2"] {
            assert!(
                validate_main_coder_engine_id(ok).is_ok(),
                "{ok:?} must be accepted"
            );
        }
        // A newline could inject a premature `---`/frontmatter key — must be rejected.
        assert!(
            validate_main_coder_engine_id("codex\n---\nstatus: pwned").is_err(),
            "a newline-bearing id must be rejected (frontmatter injection)"
        );
        assert!(
            validate_main_coder_engine_id("a\rb").is_err(),
            "a carriage return must be rejected"
        );
        // Belt-and-suspenders: even if a control-char value reached the serializer, the emitted
        // line must never contain a raw newline that breaks the single-line invariant.
        let line = main_coder_frontmatter_line(&Some("codex".into()));
        assert_eq!(line, "main_coder: codex\n");
        assert_eq!(
            line.matches('\n').count(),
            1,
            "exactly one trailing newline"
        );
    }

    #[test]
    fn project_markdown_roundtrip_preserves_state_block() {
        let metadata = ProjectMetadata {
            id: "orasis-pipeline".into(),
            title: "Orasis Pipeline".into(),
            status: "active".into(),
            updated_at: "2026-05-28T00:00:00Z".into(),
            root_path: Some("C:\\Users\\gualt\\Desktop\\aspis bio".into()),
            censor_trusted: false,
            net_enabled: false,
            sandbox_mode: crate::backend::broker::SandboxMode::default(),
            working_set: Vec::new(),
            agent_controls: Default::default(),
            main_coder: None,
        };
        let state = ProjectStateBlock {
            version: 1,
            tasks: vec![ProjectTask {
                id: "T1".into(),
                title: "Deploy worker".into(),
                status: "todo".into(),
                priority: Some("high".into()),
                assignee: None,
                due: None,
                updated_at: "2026-05-28T00:00:00Z".into(),
                category: Some("bug".into()),
                description: Some("Worker returns 500 on cold start".into()),
                suspect_file_ids: vec!["src/worker.ts".into(), "src/db.ts".into()],
                depends_on: Vec::new(),
                scope: Vec::new(),
                acceptance: String::new(),
                plan_id: None,
                weight: String::new(),
            }],
            notes: Vec::new(),
            milestones: Vec::new(),
        };
        let markdown = initial_project_markdown(&metadata, &state).unwrap();
        let (parsed, _) = parse_state_block(&markdown).unwrap();

        assert_eq!(parsed.tasks[0].id, "T1");
        assert_eq!(parsed.tasks[0].title, "Deploy worker");
        // New P1 fields round-trip through the ```aspis-project``` JSON block.
        assert_eq!(parsed.tasks[0].category.as_deref(), Some("bug"));
        assert_eq!(
            parsed.tasks[0].description.as_deref(),
            Some("Worker returns 500 on cold start")
        );
        assert_eq!(
            parsed.tasks[0].suspect_file_ids,
            vec!["src/worker.ts".to_string(), "src/db.ts".to_string()]
        );
    }

    #[test]
    fn old_task_block_without_category_loads_with_defaults() {
        // A project file written before categories existed: the task object has
        // none of category/description/suspectFileIds. #[serde(default)] must
        // load it with category=None, description=None, suspect_file_ids=[].
        let markdown = r#"---
id: legacy-project
title: Legacy Project
status: active
updated_at: 2026-05-28T00:00:00Z
---

```aspis-project
{
  "version": 1,
  "tasks": [
    {
      "id": "T1",
      "title": "Old task",
      "status": "todo",
      "priority": null,
      "assignee": null,
      "due": null,
      "linkedResources": [],
      "updatedAt": "2026-05-28T00:00:00Z"
    }
  ],
  "notes": []
}
```
"#;

        let (parsed, _) = parse_state_block(markdown).unwrap();

        assert_eq!(parsed.tasks[0].id, "T1");
        assert_eq!(parsed.tasks[0].category, None);
        assert_eq!(parsed.tasks[0].description, None);
        assert!(parsed.tasks[0].suspect_file_ids.is_empty());
    }

    // ---- Phase F: calendar milestones --------------------------------------

    /// A serialized state block carrying milestones must parse back to the exact
    /// same milestones (lossless round-trip through the ```aspis-project``` JSON).
    #[test]
    fn project_milestones_roundtrip_through_state_block() {
        let metadata = ProjectMetadata {
            id: "release-train".into(),
            title: "Release Train".into(),
            status: "active".into(),
            updated_at: "2026-06-01T00:00:00Z".into(),
            root_path: None,
            censor_trusted: false,
            net_enabled: false,
            sandbox_mode: crate::backend::broker::SandboxMode::default(),
            working_set: Vec::new(),
            agent_controls: Default::default(),
            main_coder: None,
        };
        let state = ProjectStateBlock {
            version: 1,
            tasks: Vec::new(),
            notes: Vec::new(),
            milestones: vec![
                ProjectMilestone {
                    id: "M1".into(),
                    title: "Beta cut".into(),
                    date: "2026-07-15".into(),
                    note: Some("Feature freeze the week before.".into()),
                },
                ProjectMilestone {
                    id: "M2".into(),
                    title: "GA".into(),
                    date: "2026-09-01".into(),
                    note: None,
                },
            ],
        };
        let markdown = initial_project_markdown(&metadata, &state).unwrap();
        let (parsed, _) = parse_state_block(&markdown).unwrap();

        assert_eq!(parsed.milestones.len(), 2);
        assert_eq!(parsed.milestones[0].id, "M1");
        assert_eq!(parsed.milestones[0].title, "Beta cut");
        assert_eq!(parsed.milestones[0].date, "2026-07-15");
        assert_eq!(
            parsed.milestones[0].note.as_deref(),
            Some("Feature freeze the week before.")
        );
        assert_eq!(parsed.milestones[1].id, "M2");
        assert_eq!(parsed.milestones[1].note, None);
    }

    /// FORWARD-COMPAT: an OLD project file with NO `milestones` key in its state
    /// block must load with an empty milestones list (no error). This is the #1
    /// back-compat risk — old-code-written files must still parse.
    #[test]
    fn old_state_block_without_milestones_loads_empty() {
        let markdown = r#"---
id: legacy-cal
title: Legacy Cal
status: active
updated_at: 2026-05-28T00:00:00Z
---

```aspis-project
{
  "version": 1,
  "tasks": [],
  "notes": []
}
```
"#;
        let (parsed, _) = parse_state_block(markdown).unwrap();
        assert!(parsed.milestones.is_empty());
    }

    /// BLOCKER 1 — NO-CHURN INVARIANT: a project state block with NO milestones,
    /// deserialized then re-serialized with the SAME serializer the locked write
    /// uses (`to_string_pretty`), must NOT contain a `"milestones"` key. Without
    /// `skip_serializing_if = "Vec::is_empty"` the first mutate of any pre-milestone
    /// project would inject `"milestones": []`, changing the content hash/revision,
    /// marking the file git-dirty and triggering a no-op Oracle re-index. This keeps
    /// the on-disk JSON byte-stable vs the pre-milestone shape.
    #[test]
    fn empty_milestones_are_not_serialized_no_churn() {
        let block = ProjectStateBlock {
            version: 1,
            tasks: Vec::new(),
            notes: Vec::new(),
            milestones: Vec::new(),
        };
        let json = serde_json::to_string_pretty(&block).unwrap();
        assert!(
            !json.contains("milestones"),
            "an empty milestones list must NOT be serialized (would churn old files): {json}"
        );
        // A populated list IS serialized (regression guard the skip didn't go too far).
        let populated = ProjectStateBlock {
            version: 1,
            tasks: Vec::new(),
            notes: Vec::new(),
            milestones: vec![ProjectMilestone {
                id: "M1".into(),
                title: "Ship".into(),
                date: "2026-07-15".into(),
                note: None,
            }],
        };
        assert!(serde_json::to_string_pretty(&populated)
            .unwrap()
            .contains("milestones"));
    }

    /// MAJOR 2 — LOAD NORMALIZATION: a hand-edited / externally-written state block
    /// with a duplicate milestone id, a malformed date, and an empty title must load
    /// (degrade, not error) with the bad entries DROPPED and ids DEDUPED (keep
    /// first), mirroring the task category degrade pattern. Prevents React key
    /// collisions and stale entries persisting forever.
    #[test]
    fn validate_project_state_normalizes_milestones_on_load() {
        let markdown = r#"---
id: dirty-cal
title: Dirty Cal
status: active
updated_at: 2026-05-28T00:00:00Z
---

```aspis-project
{
  "version": 1,
  "tasks": [],
  "notes": [],
  "milestones": [
    { "id": "Mdup", "title": "First", "date": "2026-07-15" },
    { "id": "Mdup", "title": "Duplicate id", "date": "2026-07-16" },
    { "id": "Mbad", "title": "Bad date", "date": "2026-13-40" },
    { "id": "Mempty", "title": "   ", "date": "2026-08-01" },
    { "id": "Mok", "title": "Keeper", "date": "2026-09-01" }
  ]
}
```
"#;
        let (mut state, _) = parse_state_block(markdown).unwrap();
        validate_project_state(&mut state).expect("a dirty milestone list must degrade, not error");
        let ids: Vec<&str> = state.milestones.iter().map(|m| m.id.as_str()).collect();
        // Duplicate id deduped (first kept), malformed-date + empty-title dropped.
        assert_eq!(ids, vec!["Mdup", "Mok"]);
        assert_eq!(state.milestones[0].title, "First");
    }

    /// FORWARD-COMPAT: a hand-added / NEW-style file that DOES carry a milestones
    /// section still parses, and a milestone survives a full write→read cycle on a
    /// freshly serialized block (proves the new schema stays parseable + lossless).
    #[test]
    fn new_state_block_with_milestones_section_parses() {
        let markdown = r#"---
id: new-cal
title: New Cal
status: active
updated_at: 2026-05-28T00:00:00Z
---

```aspis-project
{
  "version": 1,
  "tasks": [],
  "notes": [],
  "milestones": [
    { "id": "M9", "title": "Audit", "date": "2026-08-10", "note": "Whole-diff review." }
  ]
}
```
"#;
        let (parsed, _) = parse_state_block(markdown).unwrap();
        assert_eq!(parsed.milestones.len(), 1);
        assert_eq!(parsed.milestones[0].id, "M9");
        assert_eq!(parsed.milestones[0].date, "2026-08-10");
        assert_eq!(
            parsed.milestones[0].note.as_deref(),
            Some("Whole-diff review.")
        );
    }

    /// A milestone with no `note` key still parses (the field is serde-default).
    #[test]
    fn milestone_without_note_key_parses() {
        let markdown = r#"---
id: nonote-cal
title: NoNote
status: active
updated_at: 2026-05-28T00:00:00Z
---

```aspis-project
{
  "version": 1,
  "tasks": [],
  "notes": [],
  "milestones": [ { "id": "M1", "title": "Cut", "date": "2026-08-10" } ]
}
```
"#;
        let (parsed, _) = parse_state_block(markdown).unwrap();
        assert_eq!(parsed.milestones[0].note, None);
    }

    /// FIX 1: `validate_project_state` is the single READ choke point — it MUST
    /// normalize each task's `category` (trim + lowercase) so a hand-edited
    /// `"category": "Bug"` loads as `Some("bug")` and IS collected by
    /// `collect_open_bug_suspects` (whose filter is `== Some("bug")`). Otherwise a
    /// legitimately-categorized bug card would silently raise NO Polis smoke.
    #[test]
    fn validate_project_state_lowercases_hand_edited_category() {
        let mut state = ProjectStateBlock {
            version: 1,
            tasks: vec![ProjectTask {
                id: "T1".into(),
                category: Some("Bug".into()),
                suspect_file_ids: vec!["src/worker.ts".into()],
                ..task("todo")
            }],
            notes: Vec::new(),
            milestones: Vec::new(),
        };
        validate_project_state(&mut state).expect("a hand-edited mixed-case category must load");
        assert_eq!(state.tasks[0].category.as_deref(), Some("bug"));
        // And it now passes the bug-only suspect filter end-to-end.
        let collected = collect_open_bug_suspects(&state.tasks);
        assert_eq!(
            collected,
            vec![("T1".to_string(), vec!["src/worker.ts".to_string()])]
        );
    }

    /// FIX 1: an INVALID category in a legacy/hand-edited file must NOT brick the
    /// whole project load — it DEGRADES to `None` (uncategorized) rather than
    /// erroring, so a single bad value never makes an entire project unreadable. A
    /// degraded card simply raises no smoke (it is no longer a bug card).
    #[test]
    fn validate_project_state_degrades_invalid_category_to_none() {
        let mut state = ProjectStateBlock {
            version: 1,
            tasks: vec![ProjectTask {
                id: "T1".into(),
                category: Some("banana".into()),
                suspect_file_ids: vec!["src/worker.ts".into()],
                ..task("todo")
            }],
            notes: Vec::new(),
            milestones: Vec::new(),
        };
        validate_project_state(&mut state)
            .expect("an invalid category must degrade, not error the load");
        assert_eq!(state.tasks[0].category, None);
        // A degraded (now uncategorized) card raises no smoke.
        assert!(collect_open_bug_suspects(&state.tasks).is_empty());
    }

    #[test]
    fn project_state_block_ignores_fences_inside_json_strings() {
        let markdown = r#"---
id: fence-test
title: Fence Test
status: active
updated_at: 2026-05-28T00:00:00Z
---

```aspis-project
{
  "version": 1,
  "tasks": [],
  "notes": [
    {
      "id": "N1",
      "text": "agent note with ``` inside text",
      "source": "test",
      "createdAt": "2026-05-28T00:00:00Z"
    }
  ]
}
```
"#;

        let (parsed, _) = parse_state_block(markdown).unwrap();

        assert_eq!(parsed.notes[0].text, "agent note with ``` inside text");
    }

    #[test]
    fn project_frontmatter_id_must_match_filename() {
        let (root, _path) = write_temp_project("scrna-seq");
        let path = root.join("scrna-seq.md");
        // Overwrite with a mismatched id.
        fs::write(
            &path,
            "---\nid: other-project\ntitle: Broken\nstatus: active\nupdated_at: 2026-05-28T00:00:00Z\n---\n\n```aspis-project\n{\"version\":1,\"tasks\":[],\"notes\":[]}\n```\n",
        )
        .unwrap();

        let error = match read_project_file(&path) {
            Ok(_) => panic!("frontmatter mismatch should fail"),
            Err(error) => error,
        };

        assert!(error.contains("filename expects"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn project_state_rejects_duplicate_task_ids() {
        let mut state = ProjectStateBlock {
            version: 1,
            tasks: vec![
                ProjectTask {
                    id: "T1".into(),
                    ..task("todo")
                },
                ProjectTask {
                    id: "T1".into(),
                    ..task("review")
                },
            ],
            notes: Vec::new(),
            milestones: Vec::new(),
        };

        let error = validate_project_state(&mut state).unwrap_err();

        assert!(error.contains("Duplicate project task id"));
    }

    #[test]
    fn try_read_project_file_locked_briefly_skips_fast_when_contended() {
        use fs2::FileExt;
        use std::fs::OpenOptions;
        use std::time::Instant;

        let (root, path) = write_temp_project("brief-lock-contended");

        // Hold the SAME advisory lock the reader will try to take.
        let lock_path = project_lock_path(&path);
        let holder = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&lock_path)
            .unwrap();
        holder.try_lock_exclusive().unwrap();

        let start = Instant::now();
        let result = try_read_project_file_locked_briefly(&path);
        let elapsed = start.elapsed();

        let describe = |r: &Result<Option<ParsedProject>, String>| match r {
            Ok(Some(_)) => "Ok(Some)".to_string(),
            Ok(None) => "Ok(None)".to_string(),
            Err(e) => format!("Err({e})"),
        };
        assert!(
            matches!(result, Ok(None)),
            "contended brief read must skip with Ok(None), got {}",
            describe(&result)
        );
        assert!(
            elapsed < Duration::from_millis(500),
            "single-try brief read must give up immediately under contention (no sleep), took {elapsed:?}"
        );

        // Release the lock; the brief reader now succeeds (parses the project).
        fs2::FileExt::unlock(&holder).unwrap();
        let ok = try_read_project_file_locked_briefly(&path);
        assert!(
            matches!(ok, Ok(Some(_))),
            "uncontended brief read must return the parsed project, got {}",
            describe(&ok)
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// FIX 5: a project `.md` deleted between the dir listing and the brief lock
    /// must read as `Ok(None)` (a benign "nothing this cycle"), NOT
    /// `Err("Project not found.")`.
    #[test]
    fn try_read_project_file_locked_briefly_returns_none_for_missing_file() {
        let (root, path) = write_temp_project("brief-lock-missing");
        // Delete the project file but keep the dir (mirrors a TOCTOU deletion).
        fs::remove_file(&path).unwrap();

        let result = try_read_project_file_locked_briefly(&path);
        let describe = |r: &Result<Option<ParsedProject>, String>| match r {
            Ok(Some(_)) => "Ok(Some)".to_string(),
            Ok(None) => "Ok(None)".to_string(),
            Err(e) => format!("Err({e})"),
        };
        assert!(
            matches!(result, Ok(None)),
            "a missing project file must read as Ok(None), got {}",
            describe(&result)
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// B5: the working folder must SURVIVE on disk through the serialize → write →
    /// parse round-trip (the project `.md` is the durable source of truth; the app
    /// auto-lock is in-memory auth and never rewrites the file). This pins that the
    /// frontmatter serializer emits `root_path` and the parser reads it back intact,
    /// so a re-lock/reload cannot silently drop a chosen folder to null.
    #[test]
    fn root_path_survives_markdown_round_trip() {
        let root = std::env::temp_dir().join(format!(
            "aspis-b5-rootpath-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let chosen = root.to_string_lossy().into_owned();
        let metadata = ProjectMetadata {
            id: "b5-roundtrip".into(),
            title: "B5 round trip".into(),
            status: "active".into(),
            updated_at: "2026-06-24T00:00:00Z".into(),
            root_path: Some(chosen.clone()),
            censor_trusted: false,
            net_enabled: false,
            sandbox_mode: crate::backend::broker::SandboxMode::default(),
            working_set: Vec::new(),
            agent_controls: Default::default(),
            main_coder: None,
        };
        let state = ProjectStateBlock {
            version: 1,
            tasks: Vec::new(),
            notes: Vec::new(),
            milestones: Vec::new(),
        };
        let path = root.join("b5-roundtrip.md");
        fs::write(&path, initial_project_markdown(&metadata, &state).unwrap()).unwrap();

        let parsed = read_project_file(&path).unwrap();
        assert_eq!(
            parsed.metadata.root_path,
            Some(chosen),
            "the chosen working folder must round-trip through the .md intact"
        );
        let _ = fs::remove_dir_all(&root);
    }
}
