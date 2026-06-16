//! The LOCAL planner loop (Phase 11.2): the real `plan` routine behind the
//! orchestrator's stubbed `plan` action.
//!
//! The orchestrator runs a SMALL local model. It CANNOT hold a whole codebase in
//! context, so planning is deliberately staged into three bounded phases, each
//! feeding the next a COMPACT summary rather than raw files:
//!
//! 1. **STRUCTURE** (no LLM): ask the Oracle MCP server's `project_structure`
//!    tool for the deterministic architectural spine (the handful of central
//!    files), cap it to [`MAX_SPINE`].
//! 2. **EXPLORE** (a bounded LLM loop): for EACH spine file, ONE model call whose
//!    context is SMALL — that one file's content (read locally, capped to
//!    [`MAX_EXPLORE_FILE_CHARS`]) plus optionally one grounding snippet from
//!    `oracle_context`. The model emits a STRUCTURED NOTE (a fenced JSON block);
//!    we strict-parse + bound it and accumulate the notes. Crucially each EXPLORE
//!    call is a FRESH, small context — file bodies are NEVER carried across calls,
//!    so the local model never sees more than ~one file at a time.
//! 3. **PLAN** (a SINGLE LLM call): the prompt is the goal plus the accumulated
//!    NOTES plus the structure summary — NOT raw files. The model emits a
//!    [`TasksPlan`] fenced JSON block; we strict-VALIDATE it (scope cap,
//!    acceptance, unique ids, acyclic `dependsOn` DAG, task-count cap) and, on
//!    failure, re-prompt with the precise error up to [`MAX_PLAN_ATTEMPTS`].
//!
//! Then we SUBMIT: render the plan as human-readable markdown, call the MCP
//! `plan_submit` tool (which BLOCKS for human approval and returns
//! `approved`/`rejected`/`timeout`), and PERSIST the validated `tasks.json` under
//! the project root (`.devboule/tasks.json`) so Phase 11.3's DAG runner can
//! consume it.
//!
//! Reuse, not reinvention: the per-file EXPLORE notes and the PLAN output reuse
//! the EXACT "model emits ONE fenced JSON block, Rust strict-parses + validates"
//! discipline of [`crate::action`], and every `scope` / `contextFiles` path is run
//! through the SAME safe-relative-path validator [`crate::action::check_rel_path`].
//! The DAG runner / `spawn_mini` execution is Phase 11.3 and OUT OF SCOPE here.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::action::check_rel_path;
use crate::agent_loop::Transcript;
use crate::executor::{FsBackend, McpBackend};
use crate::model::CoderModel;

// --- Bounds ------------------------------------------------------------------
// The whole point of the staging is that a LOCAL model never holds the codebase.
// These caps keep every model call SMALL and the whole routine bounded.

/// Max number of spine files we EXPLORE. The structure tool already ranks the
/// spine by centrality (in-degree); we take the top few so a large project does
/// not turn into dozens of LLM calls.
pub const MAX_SPINE: usize = 8;

/// Max CHARS of a spine file's body fed into ONE EXPLORE call. A file larger than
/// this is truncated with a marker — the model sees the head, never the whole
/// blob, and the per-call context stays small.
pub const MAX_EXPLORE_FILE_CHARS: usize = 10_000;

/// Max CHARS of the optional `oracle_context` grounding snippet appended to an
/// EXPLORE call. Tight: the file body is the primary context; the snippet is a
/// small grounding aid.
pub const MAX_GROUNDING_CHARS: usize = 2_000;

/// Hard ceiling on the TOTAL context (chars) of a single EXPLORE prompt. Belt and
/// suspenders over the per-part caps: even if a future edit grows a part, no
/// single EXPLORE call may exceed this — the proof that the local model never sees
/// the whole codebase in one shot.
pub const MAX_EXPLORE_PROMPT_CHARS: usize = 20_000;

/// Max CHARS of one accumulated EXPLORE note (the model-emitted structured note,
/// re-rendered). Bounds the PLAN prompt regardless of how verbose the notes are.
pub const MAX_NOTE_CHARS: usize = 1_500;

/// Max CHARS of ALL accumulated notes carried into the PLAN call. The PLAN prompt
/// is goal + notes + summary; this caps the notes contribution so the PLAN call is
/// also bounded.
pub const MAX_NOTES_TOTAL_CHARS: usize = 12_000;

/// Max CHARS of the structure `summary` (raw Oracle JSON) appended to the PLAN
/// prompt. The summary is untrusted server output and can be arbitrarily large;
/// cap it so it cannot balloon the single PLAN call.
pub const MAX_SUMMARY_CHARS: usize = 4_000;

/// Max CHARS of the `goal` (the joined plan steps) appended to the PLAN prompt.
/// `action.rs` caps each step but NOT the number of steps, so the joined goal is
/// unbounded without this cap.
pub const MAX_GOAL_CHARS: usize = 4_000;

/// Max CHARS of the `prior_error` (a validation/serde error) prepended to a PLAN
/// retry prompt. A serde error can quote a large model blob; keep the feedback
/// tight so a malformed prior plan cannot inflate the next prompt.
pub const MAX_PRIOR_ERROR_CHARS: usize = 1_000;

/// Hard ceiling on the TOTAL context (chars) of a single PLAN prompt. Belt and
/// suspenders over the per-part caps (goal / notes / summary / prior_error): even
/// if a future edit grows a part, no single PLAN call may exceed this — the proof
/// that the local model never sees the whole codebase in one shot, mirroring
/// [`MAX_EXPLORE_PROMPT_CHARS`] for the EXPLORE phase.
pub const MAX_PLAN_PROMPT_CHARS: usize = 24_000;

/// Max number of `key_symbols` kept from one EXPLORE note.
pub const MAX_NOTE_SYMBOLS: usize = 12;

/// Max files a single task may MODIFY. HARD CAP per the 11.3 contract: a task with
/// a tighter scope is easier to delegate and verify.
pub const MAX_TASK_SCOPE: usize = 3;

/// Max files a single task may name as read-only `contextFiles`.
pub const MAX_TASK_CONTEXT: usize = 12;

/// Max number of tasks in one plan. A plan larger than this is almost certainly
/// the model losing the thread; reject it.
pub const MAX_TASKS: usize = 40;

/// TOTAL number of PLAN attempts (the `for _ in 0..N` loop includes the FIRST try,
/// so this is attempts, not retries-after-the-first). Small: a model that cannot
/// produce a valid plan in a few tries will not on the tenth.
pub const MAX_PLAN_ATTEMPTS: usize = 3;

/// Max CHARS of any single free-text field we accept from the model (note role /
/// watch_out, task title / acceptance, plan goal). Mirrors the action layer's
/// [`crate::action::MAX_TEXT_LEN`] spirit but tighter for plan fields.
const MAX_FIELD_CHARS: usize = 2_000;

// --- The tasks.json contract (camelCase wire shape for Phase 11.3) ----------

/// The atomic plan artifact persisted as `tasks.json`. This is the CONTRACT the
/// Phase 11.3 DAG runner consumes: a flat task list with a `dependsOn` DAG.
///
/// `deny_unknown_fields`: a typo'd or extra key from the model is a hard parse
/// error (fed back as a retry message) rather than silently ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TasksPlan {
    /// The human-facing goal this plan satisfies (echoed from the orchestrator's
    /// `plan` request so 11.3 has the framing without the burst transcript).
    pub project_goal: String,
    /// The tasks, in author order. Execution order is the `dependsOn` DAG, not
    /// this vector order.
    pub tasks: Vec<Task>,
}

/// One unit of work for the 11.3 runner. `scope` is what it MODIFIES (hard-capped
/// at [`MAX_TASK_SCOPE`]); `contextFiles` are read-only deps; `acceptance` is a
/// DETERMINISTICALLY verifiable check (a test / typecheck / lint command); the
/// `dependsOn` ids wire the DAG.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Task {
    /// Unique, non-empty id (e.g. `T001`).
    pub id: String,
    /// One-line title.
    pub title: String,
    /// Files to MODIFY. HARD CAP [`MAX_TASK_SCOPE`]; each a safe relative path.
    pub scope: Vec<String>,
    /// Read-only dependency files (optional). Each a safe relative path.
    #[serde(default)]
    pub context_files: Vec<String>,
    /// A DETERMINISTICALLY verifiable acceptance check. Required, non-empty.
    pub acceptance: String,
    /// Ids of prerequisite tasks (the DAG). Each must reference an existing task
    /// id; the whole graph must be acyclic.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Kanban status. Always `"pending"` at plan time.
    pub status: String,
    /// Attempt counter. Always `0` at plan time.
    pub attempts: u32,
}

// --- The EXPLORE note (one fenced JSON block per spine file) -----------------

/// The structured NOTE the model emits for one spine file during EXPLORE. Small
/// by construction; we re-render an accumulated, bounded form into the PLAN
/// prompt. `deny_unknown_fields` so a malformed note is rejected, not absorbed.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExploreNote {
    /// The spine file this note is about. Validated to MATCH the file we asked
    /// about (a model that drifts the path is rejected).
    source: String,
    /// What the file does in the architecture.
    role: String,
    /// The key symbols it defines (capped).
    #[serde(default)]
    key_symbols: Vec<String>,
    /// A pitfall / thing to watch out for when touching it (optional).
    #[serde(default)]
    watch_out: String,
}

// --- Outcome -----------------------------------------------------------------

/// The human verdict on a submitted plan, mapped from the MCP `plan_submit`
/// status. Anything that is not an explicit approve/reject is treated as a
/// `Timeout` (the conservative outcome: do NOT proceed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanApproval {
    Approved,
    Rejected,
    Timeout,
}

impl PlanApproval {
    /// A short stable word for the compact result line.
    pub fn as_str(self) -> &'static str {
        match self {
            PlanApproval::Approved => "approved",
            PlanApproval::Rejected => "rejected",
            PlanApproval::Timeout => "timeout",
        }
    }

    /// Map a `plan_submit` status string to an approval. `approved` / `rejected`
    /// are explicit; EVERYTHING else (`timeout`, `vanished`, `not_found`,
    /// `pending_approval`, an unknown value) is the conservative `Timeout` — the
    /// orchestrator must NOT treat a non-approval as a go-ahead.
    fn from_status(status: &str) -> Self {
        match status.trim().to_ascii_lowercase().as_str() {
            "approved" => PlanApproval::Approved,
            "rejected" => PlanApproval::Rejected,
            _ => PlanApproval::Timeout,
        }
    }
}

/// What `run_planner` returns to the orchestrator: the validated plan, the human
/// verdict, and where the `tasks.json` was persisted (so 11.3 can find it).
#[derive(Debug, Clone)]
pub struct PlanOutcome {
    pub tasks_plan: TasksPlan,
    pub approval: PlanApproval,
    pub tasks_json_path: PathBuf,
}

impl PlanOutcome {
    /// The COMPACT line the executor feeds back to the burst model so the outer
    /// loop knows the outcome without re-reading the whole plan. Names the
    /// persisted `tasks.json` so the human (and Phase 11.3's runner) knows where
    /// the atomic plan landed.
    pub fn compact_summary(&self) -> String {
        format!(
            "Plan: {} task(s), submitted -> {} (tasks.json at {})",
            self.tasks_plan.tasks.len(),
            self.approval.as_str(),
            self.tasks_json_path.display()
        )
    }
}

// --- Fenced-block extraction (mirrors crate::action's discipline) ------------

/// A fenced JSON block matcher for a given info-string label (e.g. `note` /
/// `plan`). Anchored to line starts (`(?m)^`) and tolerant of CRLF, exactly like
/// [`crate::action`]'s action fence, so an inline/indented mention in prose is not
/// mistaken for the directive.
fn fenced_re(label: &str) -> Regex {
    // `label` is a fixed internal constant ("note"/"plan"), never user input, so
    // there is no injection concern; still escape defensively.
    let pat = format!(
        r"(?ms)^```{}[ \t]*\r?\n(.*?)\r?\n?^```[ \t]*$",
        regex::escape(label)
    );
    Regex::new(&pat).expect("static planner fence regex is valid")
}

/// Extract EXACTLY ONE fenced block body for `label` from `text`. Mirrors
/// [`crate::action::parse_action`]'s count discipline: zero or more-than-one is an
/// error with a precise, model-facing message the planner feeds back on retry.
fn extract_one_block<'a>(text: &'a str, label: &str) -> Result<&'a str, String> {
    let re = fenced_re(label);
    let mut it = re.captures_iter(text);
    let first = match it.next() {
        Some(c) => c,
        None => {
            return Err(format!(
                "no ```{label}``` block found; emit EXACTLY ONE fenced ```{label}``` \
                 block containing a single JSON object"
            ))
        }
    };
    if it.next().is_some() {
        return Err(format!(
            "found more than one ```{label}``` block; emit EXACTLY ONE per turn"
        ));
    }
    Ok(first.get(1).map(|m| m.as_str().trim()).unwrap_or_default())
}

// --- Field validation helpers ------------------------------------------------

/// A free-text field must be non-empty (after trim) and within the field cap.
fn check_field(field: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("`{field}` must not be empty"));
    }
    let len = value.chars().count();
    if len > MAX_FIELD_CHARS {
        return Err(format!("`{field}` too long: {len} chars (max {MAX_FIELD_CHARS})"));
    }
    Ok(())
}

/// Truncate so the RESULT is at most `cap` CHARS (never splitting a codepoint),
/// appending a marker when cut. HARD ceiling: the returned string — marker included
/// — never exceeds `cap`, so callers using this as a final prompt guard get a true
/// upper bound (the local-model context guarantee). When cut, we reserve room for
/// the marker by keeping `cap - marker_len` chars of the input; if `cap` is smaller
/// than the marker itself, we emit just the (char-truncated) marker.
fn truncate_chars(s: &str, cap: usize) -> String {
    if s.chars().count() <= cap {
        return s.to_string();
    }
    const MARKER: &str = "\n[…truncated]";
    let marker_len = MARKER.chars().count();
    if cap <= marker_len {
        // Degenerate cap: cannot fit body + marker; emit a char-bounded marker.
        return MARKER.chars().take(cap).collect();
    }
    let kept: String = s.chars().take(cap - marker_len).collect();
    format!("{kept}{MARKER}")
}

// --- STRUCTURE phase ---------------------------------------------------------

/// One spine entry parsed from the `project_structure` result.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SpineEntry {
    path: String,
    in_degree: u64,
    top_symbols: Vec<String>,
}

/// The parsed structure result: the (capped) spine + the raw summary JSON for the
/// PLAN prompt.
struct Structure {
    spine: Vec<SpineEntry>,
    summary: serde_json::Value,
}

/// Parse the `project_structure` tool's JSON text into the spine + summary, capped
/// to [`MAX_SPINE`]. The wire shape is `{spine:[{path, inDegree,
/// topReferencedSymbols}], summary:{...}}` (camelCase). A missing/empty spine is
/// an error: there is nothing to plan against.
fn parse_structure(text: &str) -> Result<Structure, String> {
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| format!("project_structure returned unparseable JSON: {e}"))?;
    let spine_raw = value
        .get("spine")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "project_structure result has no `spine` array".to_string())?;

    let mut spine = Vec::new();
    for item in spine_raw.iter().take(MAX_SPINE) {
        let path = item
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if path.trim().is_empty() {
            continue; // skip a malformed entry rather than abort
        }
        // The spine path is UNTRUSTED tool output that we (a) inject into the
        // EXPLORE prompt and (b) feed to `fs.read` and use as the expected note
        // `source`. Run it through the SAME safe-relative-path validator the action
        // layer uses (which also enforces MAX_PATH_LEN); DROP an invalid/oversized
        // entry rather than aborting the whole plan — the other spine files still
        // yield a usable plan.
        if check_rel_path("spine path", &path).is_err() {
            continue;
        }
        let in_degree = item.get("inDegree").and_then(|v| v.as_u64()).unwrap_or(0);
        let top_symbols = item
            .get("topReferencedSymbols")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        spine.push(SpineEntry { path, in_degree, top_symbols });
    }

    if spine.is_empty() {
        return Err("project_structure returned an empty spine; nothing to plan against".to_string());
    }

    let summary = value.get("summary").cloned().unwrap_or(serde_json::Value::Null);
    Ok(Structure { spine, summary })
}

// --- EXPLORE phase -----------------------------------------------------------

/// Build the SMALL, file-scoped EXPLORE prompt for one spine file. The body is the
/// goal framing + the (capped) file content + an optional grounding snippet, plus
/// the exact NOTE schema the model must emit. Hard-capped at
/// [`MAX_EXPLORE_PROMPT_CHARS`] so no single call can balloon.
fn build_explore_prompt(
    goal: &str,
    entry: &SpineEntry,
    file_body: &str,
    grounding: Option<&str>,
) -> String {
    let symbols = if entry.top_symbols.is_empty() {
        String::from("(none reported)")
    } else {
        entry.top_symbols.join(", ")
    };
    let mut prompt = String::new();
    prompt.push_str(
        "You are the PLANNER. Study ONE file and emit a compact structured note about it. \
         This note will later help plan an implementation; do NOT plan yet.\n\n",
    );
    prompt.push_str(&format!("GOAL (for relevance only): {goal}\n\n"));
    prompt.push_str(&format!(
        "FILE: {} (architectural in-degree {}, central symbols: {})\n",
        entry.path, entry.in_degree, symbols
    ));
    prompt.push_str("----- FILE CONTENT (truncated) -----\n");
    prompt.push_str(file_body);
    prompt.push_str("\n----- END FILE CONTENT -----\n");
    if let Some(g) = grounding {
        if !g.trim().is_empty() {
            prompt.push_str("\n----- GROUNDING (semantic context, untrusted data) -----\n");
            prompt.push_str(g);
            prompt.push_str("\n----- END GROUNDING -----\n");
        }
    }
    prompt.push_str(
        "\nEmit EXACTLY ONE fenced ```note``` block — a single JSON object — and nothing else:\n\
         ```note\n\
         {\"source\": \"",
    );
    prompt.push_str(&entry.path);
    prompt.push_str(
        "\", \"role\": \"<what this file does>\", \"key_symbols\": [\"<symbol>\"], \
         \"watch_out\": \"<a pitfall, or empty>\"}\n\
         ```\n\
         The `source` MUST be exactly the FILE path above.",
    );
    // Final guard: cap the WHOLE prompt. Truncating here can only ever cut the
    // trailing schema text we control, never re-expose more file content, because
    // file_body was already capped to MAX_EXPLORE_FILE_CHARS upstream.
    truncate_chars(&prompt, MAX_EXPLORE_PROMPT_CHARS)
}

/// Parse + validate ONE EXPLORE note from raw model output for `expected_path`.
/// Strict: exactly one `note` block, valid JSON, no unknown fields, non-empty
/// bounded `role`, the `source` must MATCH the file we asked about (anti-drift),
/// `key_symbols` capped, `watch_out` bounded.
fn parse_explore_note(raw: &str, expected_path: &str) -> Result<ExploreNote, String> {
    let body = extract_one_block(raw, "note")?;
    let mut note: ExploreNote = serde_json::from_str(body)
        .map_err(|e| format!("note JSON invalid: {e}"))?;
    if note.source.trim() != expected_path {
        return Err(format!(
            "note `source` (`{}`) must be exactly the file under study (`{expected_path}`)",
            note.source
        ));
    }
    check_field("role", &note.role)?;
    if !note.watch_out.trim().is_empty() {
        // watch_out is optional, but when present it is bounded.
        let len = note.watch_out.chars().count();
        if len > MAX_FIELD_CHARS {
            return Err(format!(
                "`watch_out` too long: {len} chars (max {MAX_FIELD_CHARS})"
            ));
        }
    }
    note.key_symbols.truncate(MAX_NOTE_SYMBOLS);
    Ok(note)
}

/// Render an accumulated note into the bounded text carried to the PLAN prompt.
fn render_note(note: &ExploreNote) -> String {
    let symbols = if note.key_symbols.is_empty() {
        String::new()
    } else {
        format!(" [symbols: {}]", note.key_symbols.join(", "))
    };
    let watch = if note.watch_out.trim().is_empty() {
        String::new()
    } else {
        format!(" (watch out: {})", note.watch_out.trim())
    };
    let line = format!("- {}: {}{}{}", note.source, note.role.trim(), symbols, watch);
    truncate_chars(&line, MAX_NOTE_CHARS)
}

// --- PLAN phase --------------------------------------------------------------

/// Build the PLAN prompt: goal + accumulated NOTES + the structure summary. It
/// deliberately carries NO raw file content — only the compact notes — so the
/// single PLAN call stays small. `prior_error`, when set, is the precise
/// validation failure from the previous attempt, prepended so the model can
/// self-correct.
fn build_plan_prompt(goal: &str, notes_block: &str, summary: &serde_json::Value, prior_error: Option<&str>) -> String {
    let mut prompt = String::new();
    if let Some(err) = prior_error {
        // The prior error can quote a large model blob (a serde error echoes the
        // offending JSON); cap it so retry feedback cannot inflate the prompt.
        let err = truncate_chars(err, MAX_PRIOR_ERROR_CHARS);
        prompt.push_str(&format!(
            "YOUR PREVIOUS PLAN WAS REJECTED: {err}\nFix it and emit a corrected plan.\n\n"
        ));
    }
    prompt.push_str(
        "You are the PLANNER. Produce an ATOMIC implementation plan as a DAG of small tasks.\n\n",
    );
    // The goal is `steps.join` — each step is capped by action.rs but the COUNT of
    // steps is not, so the joined goal is unbounded without this cap.
    let goal = truncate_chars(goal, MAX_GOAL_CHARS);
    prompt.push_str(&format!("GOAL: {goal}\n\n"));
    prompt.push_str("FILE NOTES (from exploring the architectural spine):\n");
    prompt.push_str(notes_block);
    prompt.push_str("\n\nPROJECT SUMMARY: ");
    // The summary is raw, untrusted Oracle JSON and can be arbitrarily large; cap
    // it before appending.
    prompt.push_str(&truncate_chars(&summary.to_string(), MAX_SUMMARY_CHARS));
    prompt.push_str("\n\nRULES:\n");
    prompt.push_str(&format!(
        "- Each task `scope` (files it MODIFIES) has AT MOST {MAX_TASK_SCOPE} entries — split larger work.\n"
    ));
    prompt.push_str("- `acceptance` MUST be a deterministically verifiable check (a test / typecheck / lint command), non-empty.\n");
    prompt.push_str("- Task `id`s are unique and non-empty (e.g. T001). `dependsOn` lists prerequisite task ids and MUST be acyclic.\n");
    prompt.push_str("- All paths are project-root-relative (no absolute, no `..`).\n");
    prompt.push_str(&format!("- At most {MAX_TASKS} tasks.\n"));
    prompt.push_str("- Every task starts with \"status\": \"pending\" and \"attempts\": 0.\n\n");
    prompt.push_str(
        "Emit EXACTLY ONE fenced ```plan``` block — a single JSON object — and nothing else:\n\
         ```plan\n\
         {\"projectGoal\": \"...\", \"tasks\": [\n\
         {\"id\": \"T001\", \"title\": \"...\", \"scope\": [\"path/a\"], \"contextFiles\": [], \
         \"acceptance\": \"cargo test passes\", \"dependsOn\": [], \"status\": \"pending\", \"attempts\": 0}\n\
         ]}\n\
         ```",
    );
    // Final guard: cap the WHOLE prompt, mirroring the EXPLORE guard. The per-part
    // caps above (goal / notes / summary / prior_error) already bound each input;
    // this is belt-and-suspenders so no single PLAN call can ever exceed the
    // ceiling — the local-model context guarantee holds end-to-end. Truncating here
    // can only cut the trailing schema text we control (it is appended last), never
    // re-expose more model/oracle input.
    truncate_chars(&prompt, MAX_PLAN_PROMPT_CHARS)
}

/// STRICT-validate a parsed [`TasksPlan`]. Returns a precise, model-facing message
/// on the first violation (so the PLAN model can be re-prompted with it). Enforces:
/// non-empty goal; ≥1 task; ≤ [`MAX_TASKS`]; per-task non-empty/bounded title +
/// acceptance; `scope` non-empty, ≤ [`MAX_TASK_SCOPE`], each a safe relative path;
/// `contextFiles` ≤ [`MAX_TASK_CONTEXT`], each a safe relative path; unique
/// non-empty ids; `status == "pending"`; `dependsOn` references EXISTING ids only
/// (no self-dep, no dangling) and the graph is ACYCLIC.
fn validate_plan(plan: &TasksPlan) -> Result<(), String> {
    check_field("projectGoal", &plan.project_goal)?;
    if plan.tasks.is_empty() {
        return Err("plan has no tasks".to_string());
    }
    if plan.tasks.len() > MAX_TASKS {
        return Err(format!(
            "too many tasks: {} (max {MAX_TASKS})",
            plan.tasks.len()
        ));
    }

    // First pass: per-task field validation + collect ids (uniqueness).
    let mut ids: HashSet<&str> = HashSet::with_capacity(plan.tasks.len());
    for task in &plan.tasks {
        if task.id.trim().is_empty() {
            return Err("a task has an empty `id`".to_string());
        }
        if !ids.insert(task.id.as_str()) {
            return Err(format!("duplicate task id `{}`", task.id));
        }
        check_field("title", &task.title)?;
        check_field("acceptance", &task.acceptance)?;

        if task.scope.is_empty() {
            return Err(format!("task `{}` has an empty `scope`", task.id));
        }
        if task.scope.len() > MAX_TASK_SCOPE {
            return Err(format!(
                "task `{}` scope has {} files (max {MAX_TASK_SCOPE})",
                task.id,
                task.scope.len()
            ));
        }
        for p in &task.scope {
            check_rel_path("scope entry", p)
                .map_err(|e| format!("task `{}`: {e}", task.id))?;
        }
        if task.context_files.len() > MAX_TASK_CONTEXT {
            return Err(format!(
                "task `{}` has {} contextFiles (max {MAX_TASK_CONTEXT})",
                task.id,
                task.context_files.len()
            ));
        }
        for p in &task.context_files {
            check_rel_path("contextFiles entry", p)
                .map_err(|e| format!("task `{}`: {e}", task.id))?;
        }
        if task.status != "pending" {
            return Err(format!(
                "task `{}` status must be \"pending\", got `{}`",
                task.id, task.status
            ));
        }
        // The retry budget belongs to the 11.3 runner: a plan-time task MUST start
        // with a clean counter, else we persist a corrupted initial retry state.
        if task.attempts != 0 {
            return Err(format!(
                "task `{}` attempts must be 0 at plan time, got {}",
                task.id, task.attempts
            ));
        }
    }

    // Second pass: dependsOn references an EXISTING id (no dangling, no self-dep,
    // no duplicates). Duplicate dep ids corrupt the 11.3 runner's in-degree
    // bookkeeping, so reject them outright rather than silently dedup.
    for task in &plan.tasks {
        let mut seen_deps: HashSet<&str> = HashSet::with_capacity(task.depends_on.len());
        for dep in &task.depends_on {
            if dep == &task.id {
                return Err(format!("task `{}` depends on itself", task.id));
            }
            if !ids.contains(dep.as_str()) {
                return Err(format!(
                    "task `{}` dependsOn references unknown task id `{dep}`",
                    task.id
                ));
            }
            if !seen_deps.insert(dep.as_str()) {
                return Err(format!(
                    "task `{}` has a duplicate dependsOn entry `{dep}`",
                    task.id
                ));
            }
        }
    }

    // Third pass: the dependsOn graph must be ACYCLIC. Kahn's algorithm — if we
    // cannot topologically order every task, a cycle exists.
    detect_cycle(plan)?;
    Ok(())
}

/// Reject a cyclic `dependsOn` graph via Kahn's algorithm. An edge `dep -> task`
/// means `task` waits on `dep`; `task`'s in-degree is its `dependsOn` count. We
/// repeatedly remove zero-in-degree nodes; if any remain, they form a cycle.
///
/// PRECONDITION: this MUST run AFTER [`validate_plan`]'s dep-validation pass, which
/// has already rejected dangling deps (every `dependsOn` entry references an
/// EXISTING task id). The in-degree counts each task's raw `depends_on.len()`; if a
/// dangling dep slipped through, that count would never decrement to zero and a
/// FALSE cycle would be reported. The `debug_assert!` below catches a future caller
/// that violates this ordering in debug builds.
fn detect_cycle(plan: &TasksPlan) -> Result<(), String> {
    // in_degree[id] = number of unresolved prerequisites of `id`.
    let mut in_degree: HashMap<&str, usize> = HashMap::with_capacity(plan.tasks.len());
    // dependents[dep] = tasks that depend on `dep` (the reverse edges).
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::with_capacity(plan.tasks.len());
    for task in &plan.tasks {
        in_degree.insert(task.id.as_str(), task.depends_on.len());
    }
    // Cheap latent-bug guard: every dep MUST reference a known task id (the
    // dangling-dep pass in `validate_plan` guarantees this). Only runs in debug.
    debug_assert!(
        plan.tasks
            .iter()
            .all(|t| t.depends_on.iter().all(|d| in_degree.contains_key(d.as_str()))),
        "detect_cycle precondition violated: a dependsOn references an unknown id \
         (validate_plan must run first)"
    );
    for task in &plan.tasks {
        for dep in &task.depends_on {
            dependents.entry(dep.as_str()).or_default().push(task.id.as_str());
        }
    }

    let mut queue: Vec<&str> = in_degree
        .iter()
        .filter(|(_, &d)| d == 0)
        .map(|(&id, _)| id)
        .collect();
    let mut resolved = 0usize;
    while let Some(id) = queue.pop() {
        resolved += 1;
        if let Some(children) = dependents.get(id) {
            for &child in children {
                if let Some(d) = in_degree.get_mut(child) {
                    *d -= 1;
                    if *d == 0 {
                        queue.push(child);
                    }
                }
            }
        }
    }
    if resolved != plan.tasks.len() {
        return Err(
            "the dependsOn graph has a cycle (it must be acyclic)".to_string(),
        );
    }
    Ok(())
}

// --- Markdown rendering (for plan_submit) ------------------------------------

/// Render a validated [`TasksPlan`] as a human-readable markdown plan for
/// `plan_submit`. This is what the HUMAN reviews; it lists each task with its
/// scope, acceptance, and dependencies.
fn render_plan_markdown(plan: &TasksPlan) -> String {
    let mut md = String::new();
    md.push_str("# Implementation plan\n\n");
    md.push_str(&format!("**Goal:** {}\n\n", plan.project_goal.trim()));
    md.push_str(&format!("{} task(s):\n\n", plan.tasks.len()));
    for task in &plan.tasks {
        md.push_str(&format!("## {} — {}\n\n", task.id, task.title.trim()));
        md.push_str(&format!("- **Modifies:** {}\n", task.scope.join(", ")));
        if !task.context_files.is_empty() {
            md.push_str(&format!("- **Reads:** {}\n", task.context_files.join(", ")));
        }
        if !task.depends_on.is_empty() {
            md.push_str(&format!("- **Depends on:** {}\n", task.depends_on.join(", ")));
        }
        md.push_str(&format!("- **Acceptance:** {}\n\n", task.acceptance.trim()));
    }
    md
}

// --- Persistence -------------------------------------------------------------

/// The project-root-relative directory + file the validated plan is persisted to.
const TASKS_DIR: &str = ".devboule";
const TASKS_FILE: &str = "tasks.json";

/// Persist the validated plan as `<project_root>/.devboule/tasks.json`, returning
/// the absolute path. ROOT-CONFINED: the path is built ONLY from the fixed
/// internal constants above joined to the (canonical) project root — never from
/// model input — and we re-confirm the resolved parent stays inside the root
/// before writing, mirroring [`FsBackend`]'s confinement posture. Writes are
/// crash-safe-ish (temp file + rename) so a partial write never corrupts an
/// existing `tasks.json`.
fn persist_tasks_json(project_root: &Path, plan: &TasksPlan) -> Result<PathBuf, String> {
    // Canonicalize the root so the confinement boundary is well-defined (the same
    // requirement FsBackend::new enforces).
    let root = project_root
        .canonicalize()
        .map_err(|e| format!("project root is not accessible: {e}"))?;
    let dir = root.join(TASKS_DIR);
    // Confinement: the directory we are about to create/write MUST be inside root.
    // Built from fixed constants, but assert it regardless (defense in depth).
    if !dir.starts_with(&root) {
        return Err("tasks dir escapes the project root".to_string());
    }
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("could not create {TASKS_DIR}: {e}"))?;

    let json = serde_json::to_string_pretty(plan)
        .map_err(|e| format!("could not serialize tasks.json: {e}"))?;

    let final_path = dir.join(TASKS_FILE);
    // Temp + atomic rename within the SAME dir so a crash mid-write cannot leave a
    // truncated tasks.json. The temp name is fixed (no model input).
    let tmp_path = dir.join(".tasks.json.tmp");
    std::fs::write(&tmp_path, json.as_bytes())
        .map_err(|e| format!("could not write tasks.json: {e}"))?;
    std::fs::rename(&tmp_path, &final_path)
        .map_err(|e| format!("could not finalize tasks.json: {e}"))?;
    Ok(final_path)
}

// --- The routine -------------------------------------------------------------

/// Run the local planner for `goal`: STRUCTURE -> EXPLORE -> PLAN -> SUBMIT.
///
/// Drives the injected `model` (small local LLM) + `mcp` (Oracle server) + `fs`
/// (root-confined reads). `project_id` is the Oracle-side project key the
/// `project_structure` / `plan_submit` tools require; `project_root` is the
/// canonical local root the plan is persisted under. Returns a [`PlanOutcome`] on
/// success, or an Escalated-style error string the caller surfaces to the burst
/// model (e.g. the model never produced a valid plan within the retry budget, the
/// structure tool failed, or `plan_submit` errored).
pub async fn run_planner(
    goal: &str,
    model: &dyn CoderModel,
    mcp: &dyn McpBackend,
    fs: &FsBackend,
    project_id: &str,
    project_root: &Path,
) -> Result<PlanOutcome, String> {
    let goal = goal.trim();
    if goal.is_empty() {
        return Err("planner needs a non-empty goal".to_string());
    }
    if project_id.trim().is_empty() {
        return Err("planner needs a project_id (DEVBOULE_PROJECT_ID not set?)".to_string());
    }

    // --- 1) STRUCTURE (no LLM) ---
    let structure_text = mcp
        .call_tool(
            "project_structure",
            serde_json::json!({ "project_id": project_id }),
        )
        .await
        .map_err(|e| format!("project_structure failed: {e}"))?;
    let structure = parse_structure(&structure_text)?;

    // --- 2) EXPLORE (bounded LLM loop, ONE small call per spine file) ---
    // We store the ALREADY-RENDERED note line (not the parsed note) so the budget
    // accounting and the final join use the exact same string — `render_note` is
    // called once per accepted note, never twice.
    let mut rendered_notes: Vec<String> = Vec::with_capacity(structure.spine.len());
    let mut notes_total = 0usize;
    for entry in &structure.spine {
        // Read the file body locally, capped — the FS backend caps to its own read
        // limit; we further cap to MAX_EXPLORE_FILE_CHARS so the per-call context
        // stays small. A read failure (file vanished, binary) is non-fatal: skip
        // that file rather than abort the whole plan. The read is a BLOCKING syscall,
        // so it runs on a `spawn_blocking` thread (a cheap `FsBackend` clone moves in
        // — one PathBuf) and never stalls the async reactor; a JoinError is treated
        // as a skip, never a panic.
        let fs_clone = fs.clone();
        let path = entry.path.clone();
        let read = tokio::task::spawn_blocking(move || fs_clone.read(&path)).await;
        let body = match read {
            Ok(Ok(b)) => truncate_chars(&b, MAX_EXPLORE_FILE_CHARS),
            _ => continue,
        };

        // Optional grounding: ONE small oracle_context snippet. Best-effort — a
        // grounding failure must not sink the EXPLORE call.
        let grounding = mcp
            .call_tool(
                "oracle_context",
                serde_json::json!({
                    "query": format!("role and key responsibilities of {}", entry.path),
                    "limit": 1,
                    "project_id": project_id,
                }),
            )
            .await
            .ok()
            .map(|g| truncate_chars(&g, MAX_GROUNDING_CHARS));

        let prompt = build_explore_prompt(goal, entry, &body, grounding.as_deref());
        // Fresh, SMALL context per call: the whole prompt is this one file (+ one
        // snippet). No prior file bodies are carried — the local model never holds
        // the codebase. `next_output` is the model seam; the burst transcript is
        // NOT reused here (a fresh single-message transcript is built per call).
        let transcript = Transcript::new(prompt);
        let raw = model.next_output(&transcript).await;
        match parse_explore_note(&raw, &entry.path) {
            Ok(note) => {
                // Render ONCE here; reuse the same string for both the budget check
                // and the final join.
                let rendered = render_note(&note);
                let rendered_len = rendered.chars().count();
                // The final `notes_block` is `rendered_notes.join("\n")`, which adds
                // ONE newline BETWEEN entries. Count that joining newline (only when
                // this is not the first note) so the budget reflects the true block
                // size, not just the sum of note bodies.
                let join_cost = usize::from(!rendered_notes.is_empty());
                if notes_total + rendered_len + join_cost > MAX_NOTES_TOTAL_CHARS {
                    // The notes budget for the PLAN prompt is full; stop exploring
                    // further files rather than overflow the single PLAN call.
                    break;
                }
                notes_total += rendered_len + join_cost;
                rendered_notes.push(rendered);
            }
            // A malformed note for ONE file is non-fatal: the plan can still be
            // produced from the other notes + the summary. Skip it.
            Err(_) => continue,
        }
    }

    // Assemble the notes block from the already-rendered lines (no re-render). A
    // final `truncate_chars` is belt-and-suspenders over the running budget so the
    // block can never exceed MAX_NOTES_TOTAL_CHARS even if the accounting drifts.
    let notes_block = if rendered_notes.is_empty() {
        "(no usable file notes; plan from the goal + summary)".to_string()
    } else {
        truncate_chars(&rendered_notes.join("\n"), MAX_NOTES_TOTAL_CHARS)
    };

    // --- 3) PLAN (a SINGLE LLM call, with bounded validation retries) ---
    let mut prior_error: Option<String> = None;
    let mut plan: Option<TasksPlan> = None;
    for _ in 0..MAX_PLAN_ATTEMPTS {
        let prompt = build_plan_prompt(goal, &notes_block, &structure.summary, prior_error.as_deref());
        let transcript = Transcript::new(prompt);
        let raw = model.next_output(&transcript).await;

        let candidate = (|| -> Result<TasksPlan, String> {
            let body = extract_one_block(&raw, "plan")?;
            let parsed: TasksPlan =
                serde_json::from_str(body).map_err(|e| format!("plan JSON invalid: {e}"))?;
            validate_plan(&parsed)?;
            Ok(parsed)
        })();

        match candidate {
            Ok(valid) => {
                plan = Some(valid);
                break;
            }
            Err(e) => prior_error = Some(e),
        }
    }
    let plan = plan.ok_or_else(|| {
        format!(
            "planner could not produce a valid plan in {MAX_PLAN_ATTEMPTS} attempts: {}",
            prior_error.unwrap_or_else(|| "unknown error".to_string())
        )
    })?;

    // --- 4) SUBMIT: persist tasks.json, then plan_submit (human gate) ---
    // Persist BEFORE submitting so the durable artifact exists even if the human
    // rejects (11.3's runner only consumes it on approval, but the record is
    // useful regardless and the persist is the cheap, local step). The persist is
    // BLOCKING fs work (canonicalize / mkdir / write / rename), so it runs on a
    // `spawn_blocking` thread rather than stalling the reactor; owned copies move
    // in and a JoinError becomes a clean error, never a panic.
    let tasks_json_path = {
        let root = project_root.to_path_buf();
        let plan_for_persist = plan.clone();
        tokio::task::spawn_blocking(move || persist_tasks_json(&root, &plan_for_persist))
            .await
            .map_err(|e| format!("tasks.json persist task failed: {e}"))??
    };

    let plan_markdown = render_plan_markdown(&plan);
    let title = format!(
        "Devboule plan: {}",
        truncate_chars(goal, 120).replace('\n', " ")
    );
    let submit_text = mcp
        .call_tool(
            "plan_submit",
            serde_json::json!({
                "project_id": project_id,
                "title": title,
                "plan_markdown": plan_markdown,
            }),
        )
        .await
        .map_err(|e| format!("plan_submit failed: {e}"))?;

    let approval = parse_submit_status(&submit_text);

    Ok(PlanOutcome {
        tasks_plan: plan,
        approval,
        tasks_json_path,
    })
}

/// Map the `plan_submit` result text to a [`PlanApproval`]. The tool returns a
/// JSON object carrying `status`; a non-JSON or status-less body is the
/// conservative `Timeout` (never a false `Approved`).
fn parse_submit_status(text: &str) -> PlanApproval {
    let parsed = serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| v.get("status").and_then(|s| s.as_str()).map(PlanApproval::from_status));
    match parsed {
        Some(approval) => approval,
        None => {
            // A non-JSON or status-less body is conservatively a Timeout, but that
            // makes a SERVER ERROR indistinguishable from a genuine human timeout.
            // Emit a bounded diagnostic (a short prefix only — never the whole
            // untrusted body, to keep logs clean) so the operator can tell them
            // apart. No behavior change: still Timeout.
            eprintln!(
                "devboule planner: plan_submit returned no usable `status` \
                 (treating as timeout); body prefix: {:?}",
                truncate_chars(text, 200)
            );
            PlanApproval::Timeout
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;
    use tokio::sync::mpsc;

    // --- A capturing scripted model -----------------------------------------
    // Replays a FIXED sequence of raw outputs (one per next_output) AND records
    // the human-message context of every call, so tests can assert (a) the number
    // of model calls, (b) that each EXPLORE prompt is small + file-scoped (never
    // the whole codebase), and (c) the PLAN prompt carries the goal + notes.

    struct CapturingModel {
        outputs: Vec<String>,
        cursor: Mutex<usize>,
        seen_prompts: Mutex<Vec<String>>,
    }
    impl CapturingModel {
        fn new(outputs: Vec<String>) -> Self {
            Self {
                outputs,
                cursor: Mutex::new(0),
                seen_prompts: Mutex::new(Vec::new()),
            }
        }
        fn prompts(&self) -> Vec<String> {
            self.seen_prompts.lock().unwrap().clone()
        }
    }
    #[async_trait]
    impl CoderModel for CapturingModel {
        async fn reply(&self, _prompt: String, _tx: mpsc::Sender<String>) {}
        async fn next_output(&self, transcript: &Transcript) -> String {
            self.seen_prompts
                .lock()
                .unwrap()
                .push(transcript.human_message().to_string());
            let next = {
                let mut cursor = self.cursor.lock().unwrap();
                let idx = *cursor;
                if idx < self.outputs.len() {
                    *cursor += 1;
                    self.outputs.get(idx).cloned()
                } else {
                    None
                }
            };
            // Past the script: emit a deliberately INVALID block so a test that
            // over-runs fails loudly rather than silently looping.
            next.unwrap_or_else(|| "EXHAUSTED".to_string())
        }
    }

    // --- A recording mock MCP backend ---------------------------------------
    // Returns a fixed spine for `project_structure`, a canned snippet for
    // `oracle_context`, and a configurable status for `plan_submit`; records every
    // call so tests can assert the tool sequence + the plan_submit payload.

    struct MockMcp {
        spine_paths: Vec<String>,
        submit_status: String,
        calls: Mutex<Vec<(String, serde_json::Value)>>,
    }
    impl MockMcp {
        fn new(spine_paths: Vec<&str>, submit_status: &str) -> Self {
            Self {
                spine_paths: spine_paths.into_iter().map(|s| s.to_string()).collect(),
                submit_status: submit_status.to_string(),
                calls: Mutex::new(Vec::new()),
            }
        }
        fn calls(&self) -> Vec<(String, serde_json::Value)> {
            self.calls.lock().unwrap().clone()
        }
        fn call_names(&self) -> Vec<String> {
            self.calls().into_iter().map(|(n, _)| n).collect()
        }
        fn last_of(&self, name: &str) -> Option<serde_json::Value> {
            self.calls()
                .into_iter()
                .rev()
                .find(|(n, _)| n == name)
                .map(|(_, p)| p)
        }
    }
    #[async_trait]
    impl McpBackend for MockMcp {
        async fn call_tool(&self, name: &str, params: serde_json::Value) -> Result<String, String> {
            self.calls.lock().unwrap().push((name.to_string(), params.clone()));
            match name {
                "project_structure" => {
                    let spine: Vec<serde_json::Value> = self
                        .spine_paths
                        .iter()
                        .enumerate()
                        .map(|(i, p)| {
                            serde_json::json!({
                                "path": p,
                                "inDegree": (self.spine_paths.len() - i) as u64,
                                "topReferencedSymbols": ["Foo", "bar"],
                            })
                        })
                        .collect();
                    Ok(serde_json::json!({
                        "projectId": "proj",
                        "spine": spine,
                        "summary": {"scanned": 10, "capped": false},
                    })
                    .to_string())
                }
                "oracle_context" => Ok("[grounding] semantic snippet about the file".to_string()),
                "plan_submit" => {
                    Ok(serde_json::json!({"planId": "p1", "status": self.submit_status}).to_string())
                }
                other => Err(format!("unexpected tool {other}")),
            }
        }
    }

    /// Build an FsBackend over a tempdir with the named files planted, returning
    /// the dir guard (kept alive by the caller) + the backend.
    fn fs_with_files(files: &[(&str, &str)]) -> (tempfile::TempDir, FsBackend) {
        let dir = tempfile::tempdir().unwrap();
        for (rel, body) in files {
            let path = dir.path().join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, body).unwrap();
        }
        let fs = FsBackend::new(dir.path()).unwrap();
        (dir, fs)
    }

    fn note_block(source: &str) -> String {
        format!(
            "```note\n{}\n```",
            serde_json::json!({
                "source": source,
                "role": "central module",
                "key_symbols": ["Foo", "bar"],
                "watch_out": "mind the lock",
            })
        )
    }

    fn plan_block(tasks: serde_json::Value) -> String {
        format!(
            "```plan\n{}\n```",
            serde_json::json!({"projectGoal": "do the thing", "tasks": tasks})
        )
    }

    fn one_valid_task() -> serde_json::Value {
        serde_json::json!([{
            "id": "T001",
            "title": "edit a",
            "scope": ["src/a.rs"],
            "contextFiles": ["src/b.rs"],
            "acceptance": "cargo test passes",
            "dependsOn": [],
            "status": "pending",
            "attempts": 0,
        }])
    }

    /// A note whose `role` is `role_len` chars long (within MAX_FIELD_CHARS), so a
    /// test can drive the notes-budget logic with large, predictable notes.
    fn note_block_with_role(source: &str, role_len: usize) -> String {
        format!(
            "```note\n{}\n```",
            serde_json::json!({
                "source": source,
                "role": "r".repeat(role_len),
                "key_symbols": [],
                "watch_out": "",
            })
        )
    }

    // --- Happy path: STRUCTURE -> EXPLORE -> PLAN -> persist -> submit -------

    #[tokio::test]
    async fn full_planner_run_produces_persists_and_submits() {
        let (dir, fs) = fs_with_files(&[
            ("src/a.rs", "fn a() {}\n"),
            ("src/b.rs", "fn b() {}\n"),
        ]);
        let mcp = MockMcp::new(vec!["src/a.rs", "src/b.rs"], "approved");
        // One EXPLORE note per spine file (2), then one PLAN block.
        let model = CapturingModel::new(vec![
            note_block("src/a.rs"),
            note_block("src/b.rs"),
            plan_block(one_valid_task()),
        ]);

        let outcome = run_planner("do the thing", &model, &mcp, &fs, "proj", dir.path())
            .await
            .expect("planner succeeds");

        // (1) calls project_structure FIRST.
        let names = mcp.call_names();
        assert_eq!(names.first().map(|s| s.as_str()), Some("project_structure"));
        // (6) calls plan_submit (and it is the LAST call).
        assert_eq!(names.last().map(|s| s.as_str()), Some("plan_submit"));

        // (2) ONE bounded EXPLORE model call PER spine file (2 files) + (3) ONE
        // PLAN call = 3 model calls total.
        let prompts = model.prompts();
        assert_eq!(prompts.len(), 3, "2 explore + 1 plan model calls");

        // Each EXPLORE prompt is FILE-SCOPED + small: it names exactly its one
        // file, contains that file's body, and never both files at once (proof the
        // local model is not handed the whole codebase in one call).
        let explore_a = &prompts[0];
        let explore_b = &prompts[1];
        assert!(explore_a.contains("src/a.rs") && explore_a.contains("fn a()"));
        assert!(!explore_a.contains("fn b()"), "explore A must not carry B's body");
        assert!(explore_b.contains("src/b.rs") && explore_b.contains("fn b()"));
        assert!(!explore_b.contains("fn a()"), "explore B must not carry A's body");
        assert!(
            explore_a.chars().count() <= MAX_EXPLORE_PROMPT_CHARS,
            "explore prompt is bounded"
        );

        // The PLAN prompt carries the goal + the accumulated notes, NOT raw file
        // bodies.
        let plan_prompt = &prompts[2];
        assert!(plan_prompt.contains("do the thing"), "plan prompt carries the goal");
        assert!(plan_prompt.contains("central module"), "plan prompt carries the notes");
        assert!(!plan_prompt.contains("fn a()"), "plan prompt carries NO raw file body");

        // (4) a VALID TasksPlan with the expected task.
        assert_eq!(outcome.tasks_plan.tasks.len(), 1);
        assert_eq!(outcome.tasks_plan.tasks[0].id, "T001");
        assert_eq!(outcome.approval, PlanApproval::Approved);

        // (5) persisted tasks.json at <root>/.devboule/tasks.json, round-trippable.
        assert!(outcome.tasks_json_path.ends_with(".devboule/tasks.json"));
        let persisted = std::fs::read_to_string(&outcome.tasks_json_path).unwrap();
        let reparsed: TasksPlan = serde_json::from_str(&persisted).unwrap();
        assert_eq!(reparsed, outcome.tasks_plan, "persisted json round-trips");
        // camelCase on the wire (the 11.3 contract).
        assert!(persisted.contains("projectGoal"));
        assert!(persisted.contains("contextFiles"));
        assert!(persisted.contains("dependsOn"));

        // The plan_submit payload carries project_id + title + plan_markdown.
        let submit = mcp.last_of("plan_submit").unwrap();
        assert_eq!(submit["project_id"], serde_json::json!("proj"));
        assert!(submit["title"].as_str().unwrap().contains("Devboule plan"));
        assert!(submit["plan_markdown"].as_str().unwrap().contains("T001"));
    }

    #[tokio::test]
    async fn rejected_status_maps_to_rejected_but_still_persists() {
        let (dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs"], "rejected");
        let model = CapturingModel::new(vec![note_block("src/a.rs"), plan_block(one_valid_task())]);

        let outcome = run_planner("g", &model, &mcp, &fs, "proj", dir.path())
            .await
            .expect("planner succeeds even when the human rejects");
        assert_eq!(outcome.approval, PlanApproval::Rejected);
        // The tasks.json is persisted regardless of the human verdict.
        assert!(outcome.tasks_json_path.exists());
    }

    #[tokio::test]
    async fn unknown_submit_status_is_conservative_timeout() {
        // Any non-approved/non-rejected status (e.g. a server "vanished") must map
        // to Timeout — never a false Approved.
        let (dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs"], "vanished");
        let model = CapturingModel::new(vec![note_block("src/a.rs"), plan_block(one_valid_task())]);
        let outcome = run_planner("g", &model, &mcp, &fs, "proj", dir.path())
            .await
            .unwrap();
        assert_eq!(outcome.approval, PlanApproval::Timeout);
    }

    // --- EXPLORE robustness --------------------------------------------------

    #[tokio::test]
    async fn explore_note_for_missing_file_is_skipped_not_fatal() {
        // One spine file does NOT exist on disk: its EXPLORE read fails and is
        // skipped (no model call for it), but the plan still gets produced from the
        // other file's note. A skipped read means NO model call for that file.
        let (dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs", "src/ghost.rs"], "approved");
        let model = CapturingModel::new(vec![note_block("src/a.rs"), plan_block(one_valid_task())]);
        let outcome = run_planner("g", &model, &mcp, &fs, "proj", dir.path())
            .await
            .unwrap();
        // Only ONE explore call (ghost skipped) + ONE plan call.
        assert_eq!(model.prompts().len(), 2, "ghost file => no explore call for it");
        assert_eq!(outcome.tasks_plan.tasks.len(), 1);
    }

    #[tokio::test]
    async fn malformed_explore_note_is_skipped_plan_still_made() {
        // A model that emits garbage for one note must not sink the whole plan.
        let (dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs"], "approved");
        let model = CapturingModel::new(vec![
            "not a note block at all".to_string(),
            plan_block(one_valid_task()),
        ]);
        let outcome = run_planner("g", &model, &mcp, &fs, "proj", dir.path())
            .await
            .unwrap();
        assert_eq!(outcome.tasks_plan.tasks.len(), 1);
    }

    #[tokio::test]
    async fn notes_budget_accounts_for_join_newlines() {
        // Stress the notes budget with many large notes across the full spine. The
        // PLAN prompt's notes block (rendered_notes.join("\n")) must NEVER exceed
        // MAX_NOTES_TOTAL_CHARS — the join newlines BETWEEN entries are counted, so
        // the running budget can no longer under-count and overflow.
        const N: usize = MAX_SPINE; // 8 files
        // Each rendered note line is "- <source>: <role>" + truncation; pick a role
        // length that pushes several notes near the cap so the budget actually trips
        // and the join newlines matter.
        let role_len = MAX_NOTE_CHARS; // render_note truncates the line to MAX_NOTE_CHARS

        let paths: Vec<String> = (0..N).map(|i| format!("src/f{i}.rs")).collect();
        let files: Vec<(&str, &str)> = paths.iter().map(|p| (p.as_str(), "fn x() {}\n")).collect();
        let (dir, fs) = fs_with_files(&files);
        let mcp = MockMcp::new(paths.iter().map(|s| s.as_str()).collect(), "approved");

        let mut outputs: Vec<String> = paths
            .iter()
            .map(|p| note_block_with_role(p, role_len))
            .collect();
        outputs.push(plan_block(one_valid_task()));
        let model = CapturingModel::new(outputs);

        run_planner("g", &model, &mcp, &fs, "proj", dir.path())
            .await
            .expect("planner succeeds");

        // The PLAN prompt is the LAST model call. Extract its notes block and assert
        // it is bounded. We slice between the two markers build_plan_prompt uses.
        let prompts = model.prompts();
        let plan_prompt = prompts.last().unwrap();
        let start = plan_prompt
            .find("FILE NOTES (from exploring the architectural spine):\n")
            .unwrap()
            + "FILE NOTES (from exploring the architectural spine):\n".len();
        let end = plan_prompt[start..].find("\n\nPROJECT SUMMARY:").unwrap();
        let notes_block = &plan_prompt[start..start + end];
        assert!(
            notes_block.chars().count() <= MAX_NOTES_TOTAL_CHARS,
            "notes block (incl join newlines) must be <= {MAX_NOTES_TOTAL_CHARS}, got {}",
            notes_block.chars().count()
        );
        // And the whole PLAN prompt is bounded regardless.
        assert!(plan_prompt.chars().count() <= MAX_PLAN_PROMPT_CHARS);
    }

    #[tokio::test]
    async fn explore_note_with_drifted_source_is_rejected() {
        // The note's `source` must match the file under study; a drifted path is
        // rejected (and skipped), proving the anti-drift guard.
        assert!(parse_explore_note(&note_block("src/OTHER.rs"), "src/a.rs").is_err());
        assert!(parse_explore_note(&note_block("src/a.rs"), "src/a.rs").is_ok());
    }

    // --- PLAN validation (reject + retry feeds the error back) ---------------

    #[tokio::test]
    async fn four_file_scope_is_rejected_then_retry_succeeds() {
        let (dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs"], "approved");
        let four_scope = serde_json::json!([{
            "id": "T001", "title": "too big",
            "scope": ["a.rs", "b.rs", "c.rs", "d.rs"],
            "acceptance": "x", "dependsOn": [], "status": "pending", "attempts": 0,
        }]);
        let model = CapturingModel::new(vec![
            note_block("src/a.rs"),
            plan_block(four_scope),             // attempt 1: rejected (scope > 3)
            plan_block(one_valid_task()),       // attempt 2: valid
        ]);
        let outcome = run_planner("g", &model, &mcp, &fs, "proj", dir.path())
            .await
            .expect("retry produces a valid plan");
        assert_eq!(outcome.tasks_plan.tasks.len(), 1);
        // The retry prompt fed the precise error back.
        let prompts = model.prompts();
        let retry_prompt = prompts.last().unwrap();
        assert!(
            retry_prompt.contains("REJECTED") && retry_prompt.contains("scope"),
            "the retry prompt fed the scope error back: {retry_prompt}"
        );
    }

    #[tokio::test]
    async fn dependson_cycle_is_rejected() {
        let plan = TasksPlan {
            project_goal: "g".into(),
            tasks: vec![
                Task {
                    id: "T001".into(), title: "a".into(), scope: vec!["a.rs".into()],
                    context_files: vec![], acceptance: "x".into(),
                    depends_on: vec!["T002".into()], status: "pending".into(), attempts: 0,
                },
                Task {
                    id: "T002".into(), title: "b".into(), scope: vec!["b.rs".into()],
                    context_files: vec![], acceptance: "x".into(),
                    depends_on: vec!["T001".into()], status: "pending".into(), attempts: 0,
                },
            ],
        };
        let err = validate_plan(&plan).expect_err("a cycle must be rejected");
        assert!(err.contains("cycle"), "names the cycle: {err}");
    }

    #[tokio::test]
    async fn dangling_dependson_is_rejected() {
        let plan = TasksPlan {
            project_goal: "g".into(),
            tasks: vec![Task {
                id: "T001".into(), title: "a".into(), scope: vec!["a.rs".into()],
                context_files: vec![], acceptance: "x".into(),
                depends_on: vec!["T999".into()], status: "pending".into(), attempts: 0,
            }],
        };
        let err = validate_plan(&plan).expect_err("a dangling dep must be rejected");
        assert!(err.contains("unknown task id"), "names the dangling dep: {err}");
    }

    #[tokio::test]
    async fn empty_acceptance_is_rejected() {
        let plan = TasksPlan {
            project_goal: "g".into(),
            tasks: vec![Task {
                id: "T001".into(), title: "a".into(), scope: vec!["a.rs".into()],
                context_files: vec![], acceptance: "   ".into(),
                depends_on: vec![], status: "pending".into(), attempts: 0,
            }],
        };
        let err = validate_plan(&plan).expect_err("empty acceptance must be rejected");
        assert!(err.contains("acceptance"), "names acceptance: {err}");
    }

    #[tokio::test]
    async fn duplicate_ids_rejected() {
        let plan = TasksPlan {
            project_goal: "g".into(),
            tasks: vec![
                Task {
                    id: "T001".into(), title: "a".into(), scope: vec!["a.rs".into()],
                    context_files: vec![], acceptance: "x".into(),
                    depends_on: vec![], status: "pending".into(), attempts: 0,
                },
                Task {
                    id: "T001".into(), title: "b".into(), scope: vec!["b.rs".into()],
                    context_files: vec![], acceptance: "x".into(),
                    depends_on: vec![], status: "pending".into(), attempts: 0,
                },
            ],
        };
        let err = validate_plan(&plan).expect_err("duplicate ids must be rejected");
        assert!(err.contains("duplicate"), "names the duplicate: {err}");
    }

    #[tokio::test]
    async fn duplicate_dependson_entry_rejected() {
        // A model emitting the SAME prerequisite twice within one task's dependsOn
        // would corrupt the 11.3 runner's in-degree bookkeeping; reject it.
        let plan = TasksPlan {
            project_goal: "g".into(),
            tasks: vec![
                Task {
                    id: "T001".into(), title: "a".into(), scope: vec!["a.rs".into()],
                    context_files: vec![], acceptance: "x".into(),
                    depends_on: vec![], status: "pending".into(), attempts: 0,
                },
                Task {
                    id: "T002".into(), title: "b".into(), scope: vec!["b.rs".into()],
                    context_files: vec![], acceptance: "x".into(),
                    depends_on: vec!["T001".into(), "T001".into()],
                    status: "pending".into(), attempts: 0,
                },
            ],
        };
        let err = validate_plan(&plan).expect_err("a duplicate dep must be rejected");
        assert!(err.contains("duplicate dependsOn"), "names the duplicate dep: {err}");
    }

    #[tokio::test]
    async fn nonzero_attempts_rejected() {
        // A plan-time task MUST start with attempts == 0; a non-zero counter would
        // persist a corrupted initial retry state for 11.3.
        let plan = TasksPlan {
            project_goal: "g".into(),
            tasks: vec![Task {
                id: "T001".into(), title: "a".into(), scope: vec!["a.rs".into()],
                context_files: vec![], acceptance: "x".into(),
                depends_on: vec![], status: "pending".into(), attempts: 1,
            }],
        };
        let err = validate_plan(&plan).expect_err("attempts != 0 must be rejected");
        assert!(err.contains("attempts must be 0"), "names attempts: {err}");
    }

    #[tokio::test]
    async fn unsafe_scope_path_is_rejected() {
        // A `..` traversal path in scope is rejected by the shared action path
        // validator (reused, not reinvented).
        let plan = TasksPlan {
            project_goal: "g".into(),
            tasks: vec![Task {
                id: "T001".into(), title: "a".into(), scope: vec!["../escape.rs".into()],
                context_files: vec![], acceptance: "x".into(),
                depends_on: vec![], status: "pending".into(), attempts: 0,
            }],
        };
        let err = validate_plan(&plan).expect_err("a traversal scope path must be rejected");
        assert!(err.contains(".."), "names the traversal: {err}");
    }

    #[tokio::test]
    async fn self_dependency_is_rejected() {
        let plan = TasksPlan {
            project_goal: "g".into(),
            tasks: vec![Task {
                id: "T001".into(), title: "a".into(), scope: vec!["a.rs".into()],
                context_files: vec![], acceptance: "x".into(),
                depends_on: vec!["T001".into()], status: "pending".into(), attempts: 0,
            }],
        };
        let err = validate_plan(&plan).expect_err("a self-dep must be rejected");
        assert!(err.contains("itself"), "names self-dep: {err}");
    }

    #[tokio::test]
    async fn linear_dag_is_accepted() {
        // A valid linear DAG T001 -> T002 -> T003 must pass.
        let plan = TasksPlan {
            project_goal: "g".into(),
            tasks: vec![
                Task { id: "T001".into(), title: "a".into(), scope: vec!["a.rs".into()],
                    context_files: vec![], acceptance: "x".into(), depends_on: vec![],
                    status: "pending".into(), attempts: 0 },
                Task { id: "T002".into(), title: "b".into(), scope: vec!["b.rs".into()],
                    context_files: vec![], acceptance: "x".into(), depends_on: vec!["T001".into()],
                    status: "pending".into(), attempts: 0 },
                Task { id: "T003".into(), title: "c".into(), scope: vec!["c.rs".into()],
                    context_files: vec![], acceptance: "x".into(), depends_on: vec!["T002".into()],
                    status: "pending".into(), attempts: 0 },
            ],
        };
        assert!(validate_plan(&plan).is_ok(), "a linear DAG is valid");
    }

    #[tokio::test]
    async fn exhausted_retries_returns_escalation_error() {
        // Every PLAN attempt is invalid (4-file scope) -> the planner gives up
        // after MAX_PLAN_ATTEMPTS with an Escalated-style error, and never submits.
        let (dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs"], "approved");
        let bad = serde_json::json!([{
            "id": "T001", "title": "too big",
            "scope": ["a.rs", "b.rs", "c.rs", "d.rs"],
            "acceptance": "x", "dependsOn": [], "status": "pending", "attempts": 0,
        }]);
        let mut outputs = vec![note_block("src/a.rs")];
        for _ in 0..MAX_PLAN_ATTEMPTS {
            outputs.push(plan_block(bad.clone()));
        }
        let model = CapturingModel::new(outputs);
        let err = run_planner("g", &model, &mcp, &fs, "proj", dir.path())
            .await
            .expect_err("exhausted retries is an error");
        assert!(err.contains("could not produce a valid plan"), "escalation msg: {err}");
        // It must NOT have submitted a plan.
        assert!(!mcp.call_names().iter().any(|n| n == "plan_submit"));
    }

    // --- Structure parsing + small bits -------------------------------------

    #[test]
    fn parse_structure_caps_spine_to_max() {
        let spine: Vec<serde_json::Value> = (0..(MAX_SPINE + 5))
            .map(|i| serde_json::json!({"path": format!("f{i}.rs"), "inDegree": 1, "topReferencedSymbols": []}))
            .collect();
        let text = serde_json::json!({"spine": spine, "summary": {}}).to_string();
        let s = parse_structure(&text).unwrap();
        assert_eq!(s.spine.len(), MAX_SPINE, "spine capped to MAX_SPINE");
    }

    #[test]
    fn parse_structure_empty_spine_is_error() {
        let text = serde_json::json!({"spine": [], "summary": {}}).to_string();
        assert!(parse_structure(&text).is_err());
    }

    #[test]
    fn parse_structure_drops_unsafe_spine_paths_keeps_valid() {
        // Spine paths are UNTRUSTED tool output. A `..` traversal, an absolute path,
        // and an oversized path are each DROPPED (not fatal); valid entries remain.
        let huge = "a/".repeat(crate::action::MAX_PATH_LEN); // > MAX_PATH_LEN chars
        let spine = serde_json::json!([
            {"path": "src/good.rs", "inDegree": 5, "topReferencedSymbols": []},
            {"path": "../escape.rs", "inDegree": 4, "topReferencedSymbols": []},
            {"path": "/etc/passwd", "inDegree": 3, "topReferencedSymbols": []},
            {"path": huge, "inDegree": 2, "topReferencedSymbols": []},
            {"path": "src/also_good.rs", "inDegree": 1, "topReferencedSymbols": []},
        ]);
        let text = serde_json::json!({"spine": spine, "summary": {}}).to_string();
        let s = parse_structure(&text).unwrap();
        let paths: Vec<&str> = s.spine.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["src/good.rs", "src/also_good.rs"], "only safe paths kept: {paths:?}");
    }

    #[test]
    fn parse_structure_all_unsafe_spine_is_empty_error() {
        // If EVERY spine entry is dropped as unsafe, there is nothing to plan
        // against — the empty-spine error path fires.
        let spine = serde_json::json!([
            {"path": "../escape.rs", "inDegree": 4, "topReferencedSymbols": []},
            {"path": "/abs.rs", "inDegree": 3, "topReferencedSymbols": []},
        ]);
        let text = serde_json::json!({"spine": spine, "summary": {}}).to_string();
        assert!(parse_structure(&text).is_err(), "all-unsafe spine is an error");
    }

    #[test]
    fn plan_prompt_is_hard_bounded_for_giant_inputs() {
        // The local-model context guarantee: even a GIANT summary + goal + prior
        // error can never make the PLAN prompt exceed MAX_PLAN_PROMPT_CHARS, exactly
        // like the EXPLORE guard.
        let giant_goal = "step ".repeat(50_000); // ~250k chars
        let giant_notes = "x".repeat(100_000);
        let giant_summary = serde_json::json!({ "blob": "y".repeat(100_000) });
        let giant_prior = "z".repeat(50_000);
        let prompt = build_plan_prompt(&giant_goal, &giant_notes, &giant_summary, Some(&giant_prior));
        assert!(
            prompt.chars().count() <= MAX_PLAN_PROMPT_CHARS,
            "PLAN prompt must be hard-bounded: got {} chars (max {MAX_PLAN_PROMPT_CHARS})",
            prompt.chars().count()
        );
    }

    #[test]
    fn parse_structure_non_json_is_error() {
        assert!(parse_structure("not json").is_err());
    }

    #[test]
    fn missing_project_id_escalates() {
        // Validated synchronously before any tool call.
        let plan = TasksPlan { project_goal: "g".into(), tasks: vec![] };
        // (use validate to exercise empty-tasks path too)
        assert!(validate_plan(&plan).is_err());
    }

    #[test]
    fn extract_one_block_requires_exactly_one() {
        assert!(extract_one_block("no block here", "plan").is_err());
        let two = format!("{}\n{}", plan_block(one_valid_task()), plan_block(one_valid_task()));
        assert!(extract_one_block(&two, "plan").is_err());
        assert!(extract_one_block(&plan_block(one_valid_task()), "plan").is_ok());
    }

    #[test]
    fn approval_from_status_is_case_insensitive_and_conservative() {
        assert_eq!(PlanApproval::from_status("APPROVED"), PlanApproval::Approved);
        assert_eq!(PlanApproval::from_status(" rejected "), PlanApproval::Rejected);
        assert_eq!(PlanApproval::from_status("pending_approval"), PlanApproval::Timeout);
        assert_eq!(PlanApproval::from_status("whatever"), PlanApproval::Timeout);
    }

    #[tokio::test]
    async fn empty_goal_escalates() {
        let (dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs"], "approved");
        let model = CapturingModel::new(vec![]);
        let err = run_planner("   ", &model, &mcp, &fs, "proj", dir.path())
            .await
            .expect_err("an empty goal is rejected before any work");
        assert!(err.contains("non-empty goal"));
        // No tool calls at all.
        assert!(mcp.call_names().is_empty());
    }

    #[tokio::test]
    async fn empty_project_id_escalates() {
        let (dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs"], "approved");
        let model = CapturingModel::new(vec![]);
        let err = run_planner("g", &model, &mcp, &fs, "", dir.path())
            .await
            .expect_err("an empty project_id is rejected before any work");
        assert!(err.contains("project_id"));
        assert!(mcp.call_names().is_empty());
    }
}
