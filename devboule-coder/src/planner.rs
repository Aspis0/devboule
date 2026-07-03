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
//! `approved`/`rejected`/`timeout` PLUS the `planId`). ONLY on `approved` do we
//! create the plan's tasks on the project KANBAN via `project_create_plan_tasks`
//! — the single shared task store the Phase 11.3 runner reads. An unapproved or
//! rejected plan NEVER touches the board (no `.devboule/tasks.json` is written —
//! that local path is gone; the Kanban is the single source of truth).
//!
//! Reuse, not reinvention: the per-file EXPLORE notes and the PLAN output reuse
//! the EXACT "model emits ONE fenced JSON block, Rust strict-parses + validates"
//! discipline of [`crate::action`], and every `scope` / `contextFiles` path is run
//! through the SAME safe-relative-path validator [`crate::action::check_rel_path`].
//! The DAG runner / `spawn_mini` execution is Phase 11.3 and OUT OF SCOPE here.
//!
//! Worst-case call ceiling (deliberate): if both flat PLAN attempts fail, the
//! planner escalates into the hierarchical OUTLINE/EXPAND/MERGE path on top of
//! everything already spent — a single `run_planner` call can therefore reach up
//! to ~32 SEQUENTIAL model calls ([`MAX_SPINE`] + [`MAX_GOAL_SPINE`] = 12 EXPLORE,
//! [`FLAT_PLAN_ATTEMPTS`] = 2 flat PLAN, [`MAX_OUTLINE_ATTEMPTS`] = 2 OUTLINE,
//! [`MAX_MILESTONES`] × [`MAX_EXPAND_ATTEMPTS`] = 16 EXPAND). This is intentional
//! and there is NO concurrency anywhere in this module — the local model endpoint
//! has no throttling of its own, so every call (EXPLORE, flat PLAN, OUTLINE, and
//! EXPAND-per-milestone) is awaited one at a time.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::action::check_rel_path;
use crate::activity::{Activity, Node};
use crate::agent_loop::Transcript;
use crate::executor::{FsBackend, McpBackend};
use crate::model::CoderModel;
use crate::preplan::Preplan;

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
/// is goal + PRE-PLAN NOTES + notes + summary; this caps the notes contribution so
/// the PLAN call is also bounded.
///
/// REDUCED from 12_000 to 7_000 when the PRE-PLAN NOTES section
/// ([`MAX_PREPLAN_PROMPT_CHARS`], 6_000 chars) was added to the same prompt: the
/// old 12_000 left no headroom for a second 6_000-char section without leaning on
/// [`MAX_PLAN_PROMPT_CHARS`]'s final belt-and-suspenders truncate — which would
/// have silently cut the trailing RULES/schema text the model needs to emit a
/// valid ```plan``` block. At 7_000, every part at ITS OWN real cap simultaneously
/// still fits under 24_000 with headroom (see
/// `plan_prompt_fits_by_construction_with_all_parts_at_their_real_caps`).
pub const MAX_NOTES_TOTAL_CHARS: usize = 7_000;

/// Max NEW spine entries added from GOAL-driven `oracle_context` grounding (on top
/// of the purely-structural [`MAX_SPINE`]). A weak local model cannot always
/// surface which INFRA files matter for THIS goal on its own; grounding the spine
/// with the goal itself fixes that. Kept SEPARATE and small so goal files never
/// starve the structural spine — total explored files per run is at most
/// `MAX_SPINE + MAX_GOAL_SPINE` (do NOT fold this into `MAX_SPINE`'s semantics).
pub const MAX_GOAL_SPINE: usize = 4;

/// Max CHARS of the rendered PRE-PLAN NOTES section (the on-disk
/// [`crate::preplan::Preplan`] external memory) fed into the PLAN prompt. Bounded
/// like every other PLAN-prompt part so a long, crash-resumed planning session
/// still keeps the single PLAN call small.
pub const MAX_PREPLAN_PROMPT_CHARS: usize = 6_000;

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

/// TOTAL number of PLAN-phase attempts across BOTH the flat and hierarchical paths:
/// [`FLAT_PLAN_ATTEMPTS`] (= `MAX_PLAN_ATTEMPTS - 1`) flat attempts, then — the
/// ADaPT pattern, decompose ONLY on failure — a single switch to the hierarchical
/// OUTLINE/EXPAND/MERGE path ([`run_hierarchical_plan`]) in place of what would have
/// been a third identical flat try. A model that cannot produce a valid FLAT plan in
/// two tries will not on a tenth flat try either — decomposing the goal is what
/// changes the odds, not repeating the same ask.
pub const MAX_PLAN_ATTEMPTS: usize = 3;

/// Number of FLAT PLAN attempts before escalating to the hierarchical path (see
/// [`MAX_PLAN_ATTEMPTS`]). A SIMPLE goal that succeeds on its first flat attempt
/// never sees this constant at all — no size-threshold pre-escalation exists; the
/// hierarchical path is reached ONLY by two flat failures.
const FLAT_PLAN_ATTEMPTS: usize = MAX_PLAN_ATTEMPTS - 1;

/// Max number of milestones the OUTLINE stage may decompose a goal into. A weak
/// local model cannot reliably track more than a handful of milestones at once; a
/// goal that would need more is almost certainly the model losing the thread —
/// reject it rather than accept an unmanageable decomposition.
pub const MAX_MILESTONES: usize = 8;

/// TOTAL number of OUTLINE attempts — the hierarchical path's own small analogue of
/// [`MAX_PLAN_ATTEMPTS`], but for the single OUTLINE call: the first try plus ONE
/// retry with the precise validation error fed back.
pub const MAX_OUTLINE_ATTEMPTS: usize = 2;

/// TOTAL number of EXPAND attempts PER MILESTONE: the first try plus ONE retry with
/// the precise per-fragment validation error fed back. A milestone that still fails
/// after this budget hard-errors the WHOLE hierarchical plan — no partial, silently
/// incomplete plans (see [`run_hierarchical_plan`]).
pub const MAX_EXPAND_ATTEMPTS: usize = 2;

/// Max CHARS of any single free-text field we accept from the model (note role /
/// watch_out, task title / acceptance, plan goal). Mirrors the action layer's
/// [`crate::action::MAX_TEXT_LEN`] spirit but tighter for plan fields.
const MAX_FIELD_CHARS: usize = 2_000;

// --- The plan-draft shape the model emits (camelCase wire shape) -------------

/// The plan the model emits in the PLAN phase: a flat task list with a `dependsOn` DAG.
/// This is the planner's INTERNAL draft — strict-parsed + validated here, then (on
/// approval) turned into the `project_create_plan_tasks` payload that creates the tasks on
/// the Kanban. It is NOT persisted to disk anymore (the Kanban is the single task store).
///
/// `deny_unknown_fields`: a typo'd or extra key from the model is a hard parse
/// error (fed back as a retry message) rather than silently ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TasksPlan {
    /// The human-facing goal this plan satisfies (echoed from the orchestrator's
    /// `plan` request so the rendered markdown + the board create have the framing).
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
    /// ROLE UNTANGLE Phase 4: the execution TIER the runner dispatches this task
    /// to — "mini" (default; cheap/mechanical work, the one-shot mini) or "main"
    /// (the first-class Main coder: the always-agentic sandboxed engine, for
    /// substantial / multi-file / build-and-verify work). Optional in the model's
    /// JSON; an ABSENT key, an explicit `null`, or `""` all normalize to mini —
    /// local models routinely null out an unused optional, and that must not burn
    /// a plan retry (`deny_unknown_fields` on this struct only rejects UNKNOWN
    /// keys, not a null value on a known one, so the null-tolerant deserializer
    /// below is what saves it).
    #[serde(default, deserialize_with = "de_weight_null_as_empty")]
    pub weight: String,
    /// Kanban status. Always `"pending"` at plan time.
    pub status: String,
    /// Attempt counter. Always `0` at plan time.
    pub attempts: u32,
}

/// Null-tolerant string deserializer for the optional `weight` field: an explicit
/// JSON `null` deserializes to `""` (the mini default) instead of the hard
/// "invalid type: null, expected a string" error a plain `String` would raise.
/// An absent key never reaches here (`#[serde(default)]` fills `""` first).
fn de_weight_null_as_empty<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
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
/// verdict, and — on approval — the `planId` the plan's tasks were created under on
/// the Kanban (so Phase 11.3's runner can find them; `None` for a non-approved plan,
/// which never touches the board).
#[derive(Debug, Clone)]
pub struct PlanOutcome {
    pub tasks_plan: TasksPlan,
    pub approval: PlanApproval,
    /// The `planId` the plan's tasks were tagged with on the Kanban. `Some` ONLY on
    /// `approved` (the sole path that calls `project_create_plan_tasks`); `None` on a
    /// rejected/timed-out plan, which is never created on the board.
    pub plan_id: Option<String>,
}

impl PlanOutcome {
    /// The COMPACT line the executor feeds back to the burst model so the outer
    /// loop knows the outcome without re-reading the whole plan. On approval it names
    /// the `planId` the tasks were created under on the Kanban (so the human and the
    /// Phase 11.3 runner know where the plan landed); otherwise it just states the
    /// verdict (nothing was created).
    pub fn compact_summary(&self) -> String {
        match (&self.plan_id, self.approval) {
            (Some(plan_id), PlanApproval::Approved) => format!(
                "Plan: {} task(s), submitted -> approved (planId {plan_id}, created on the board)",
                self.tasks_plan.tasks.len(),
            ),
            // Approved but auto-create is OFF: the plan succeeded; its tasks were intentionally NOT
            // created (the operator will). Say so explicitly so the model does not read this like a
            // rejection nor try to run_plan tasks that don't exist.
            (None, PlanApproval::Approved) => format!(
                "Plan: {} task(s), submitted -> approved (auto-create OFF — tasks NOT created; the operator creates them)",
                self.tasks_plan.tasks.len(),
            ),
            _ => format!(
                "Plan: {} task(s), submitted -> {} (not created on the board)",
                self.tasks_plan.tasks.len(),
                self.approval.as_str(),
            ),
        }
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
        return Err(format!(
            "`{field}` too long: {len} chars (max {MAX_FIELD_CHARS})"
        ));
    }
    Ok(())
}

/// The trailing path component (basename) of a spine path, for the privacy-safe
/// EXPLORE milestone label. The spine path is forward-slash relative (Oracle output);
/// a path with no separator returns itself. An empty/`/`-terminated path falls back to
/// the whole string so a label is never empty. Basenames-only keeps the live label
/// short and leaks no directory layout into the Console.
fn path_basename(path: &str) -> &str {
    match path.rsplit('/').find(|seg| !seg.is_empty()) {
        Some(seg) => seg,
        None => path,
    }
}

/// Truncate so the RESULT is at most `cap` CHARS (never splitting a codepoint),
/// appending a marker when cut. HARD ceiling: the returned string — marker included
/// — never exceeds `cap`, so callers using this as a final prompt guard get a true
/// upper bound (the local-model context guarantee). When cut, we reserve room for
/// the marker by keeping `cap - marker_len` chars of the input; if `cap` is smaller
/// than the marker itself, we emit just the (char-truncated) marker.
pub(crate) fn truncate_chars(s: &str, cap: usize) -> String {
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
        spine.push(SpineEntry {
            path,
            in_degree,
            top_symbols,
        });
    }

    if spine.is_empty() {
        return Err(
            "project_structure returned an empty spine; nothing to plan against".to_string(),
        );
    }

    let summary = value
        .get("summary")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    Ok(Structure { spine, summary })
}

// --- oracle_context parsing (grounding, both EXPLORE snippets + GOAL spine) --

/// One retrieved chunk from the Oracle's `oracle_context` tool. Only the two
/// fields the planner actually uses are kept; NOT `deny_unknown_fields` (unlike
/// the MODEL-facing [`ExploreNote`] / [`TasksPlan`] structs) — this is SERVER
/// output whose shape we must tolerate evolving, so an unrecognized field
/// (`chunk_id`, `score`, ...) is silently ignored rather than a hard parse error.
#[derive(Debug, Clone, Deserialize)]
struct OracleChunk {
    #[serde(default)]
    file_source: String,
    #[serde(default)]
    text: String,
}

/// The `oracle_context` tool's result shape: `{query, indexStatus, chunks:[...]}`.
/// Only `chunks` matters here; everything else is ignored for the same
/// forward-compatibility reason as [`OracleChunk`].
#[derive(Debug, Clone, Deserialize)]
struct OracleContextResult {
    #[serde(default)]
    chunks: Vec<OracleChunk>,
}

/// Parse a raw `oracle_context` result into its structured shape. Tolerant: ANY
/// parse failure (non-JSON, or JSON of an unexpected shape) is `None`, never a
/// panic — the Oracle server may change shape and every caller here degrades
/// gracefully rather than breaking.
fn parse_oracle_context(raw: &str) -> Option<OracleContextResult> {
    serde_json::from_str(raw).ok()
}

/// Render a parsed `oracle_context` result as PROSE: each non-empty chunk becomes
/// `-- {file_source}\n{text}`, joined by a blank line. Before this, the RAW JSON
/// string (often half-cut by the downstream char-truncate) was fed straight to
/// the model; this is what makes the EXPLORE grounding actually readable.
fn render_oracle_chunks(result: &OracleContextResult) -> String {
    result
        .chunks
        .iter()
        .filter(|c| !c.text.trim().is_empty())
        .map(|c| format!("-- {}\n{}", c.file_source.trim(), c.text.trim()))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Build the (capped) grounding text for one `oracle_context` call: prefer the
/// structured chunk PROSE (JSON parse succeeds), else fall back to the RAW string
/// char-truncated (the pre-grounding-fix behavior) — an Oracle server that changes
/// shape must degrade the EXPLORE call, never sink it.
fn build_grounding_text(raw: &str, cap: usize) -> String {
    match parse_oracle_context(raw) {
        Some(parsed) => truncate_chars(&render_oracle_chunks(&parsed), cap),
        None => truncate_chars(raw, cap),
    }
}

/// From a GOAL-driven `oracle_context` result, extract up to [`MAX_GOAL_SPINE`]
/// NEW [`SpineEntry`] values: distinct, SAFE (per [`check_rel_path`]) `file_source`
/// paths not already in `existing` (the structural spine). Order-preserving over
/// the chunk order; an unsafe or already-known path is silently skipped — the
/// structural spine still stands on its own even if every goal chunk is unusable.
/// The entries carry no ranking data of their own (`in_degree: 0`, no symbols) —
/// unlike the structural spine, they were not scored by centrality, only by
/// semantic relevance to the goal.
///
/// ALSO filters out any path with a dot-leading component (e.g. `.devboule/...`,
/// `.git/...`) — `check_rel_path` alone does not reject these (only `..`, an
/// absolute path, or a leading `-` component), so this is an independent guard.
/// The planner must never EXPLORE a harness/tool scratchpad, least of all its OWN
/// `.devboule/preplan.md`: self-grounding on a live planning session's own notes
/// would let them leak back into that SAME session's future EXPLORE prompts as if
/// they were project source.
fn goal_spine_entries(result: &OracleContextResult, existing: &HashSet<String>) -> Vec<SpineEntry> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for chunk in &result.chunks {
        if out.len() >= MAX_GOAL_SPINE {
            break;
        }
        let path = chunk.file_source.trim();
        if path.is_empty() || existing.contains(path) || seen.contains(path) {
            continue;
        }
        if check_rel_path("goal spine path", path).is_err() {
            continue;
        }
        if has_dot_leading_component(path) {
            continue;
        }
        seen.insert(path.to_string());
        out.push(SpineEntry {
            path: path.to_string(),
            in_degree: 0,
            top_symbols: Vec::new(),
        });
    }
    out
}

/// `true` if any path component of `path` (normalizing `\` to `/` first, mirroring
/// [`check_rel_path`]'s own normalization) starts with `.` — a hidden harness/tool
/// directory (`.devboule`, `.git`, `.aspis`, ...), never a legitimate EXPLORE
/// target. `Path::components` folds a bare `.` (current-dir) to [`Component::CurDir`]
/// rather than [`Component::Normal`], so it is not mistakenly flagged here.
fn has_dot_leading_component(path: &str) -> bool {
    Path::new(&path.replace('\\', "/")).components().any(|c| {
        matches!(c, std::path::Component::Normal(name) if name.to_string_lossy().starts_with('.'))
    })
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
    let mut note: ExploreNote =
        serde_json::from_str(body).map_err(|e| format!("note JSON invalid: {e}"))?;
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
    let line = format!(
        "- {}: {}{}{}",
        note.source,
        note.role.trim(),
        symbols,
        watch
    );
    truncate_chars(&line, MAX_NOTE_CHARS)
}

/// The shared RULES block for any prompt that asks the model to emit a `tasks`
/// array — the flat PLAN prompt ([`build_plan_prompt`]) AND every hierarchical
/// EXPAND fragment prompt ([`build_expand_prompt`]) — factored into ONE helper so
/// the two prompts CANNOT drift out of sync (a rule added to one must apply to the
/// other; the flat and per-milestone task schemas are identical). `max_tasks` is
/// `Some(N)` for the flat (whole-plan) prompt, which caps the TOTAL task count in
/// this single call; the per-milestone EXPAND prompt passes `None` — a fragment has
/// no total-count rule of its own, the global [`MAX_TASKS`] cap is enforced ONCE, on
/// the fully [`merge_fragments`]-assembled plan.
fn task_rules_block(max_tasks: Option<usize>) -> String {
    let mut rules = String::new();
    rules.push_str(&format!(
        "- Each task `scope` (files it MODIFIES) has AT MOST {MAX_TASK_SCOPE} entries — split larger work.\n"
    ));
    rules.push_str("- One task = one testable, committable unit.\n");
    rules.push_str("- `acceptance` MUST be a deterministically verifiable check (a test / typecheck / lint command), non-empty.\n");
    rules.push_str("- Task `id`s are unique and non-empty (e.g. T001). `dependsOn` lists prerequisite task ids and MUST be acyclic.\n");
    rules.push_str("- All paths are project-root-relative (no absolute, no `..`).\n");
    if let Some(max_tasks) = max_tasks {
        rules.push_str(&format!("- At most {max_tasks} tasks.\n"));
    }
    rules.push_str("- Every task starts with \"status\": \"pending\" and \"attempts\": 0.\n");
    rules.push_str(
        "- Optional \"weight\": \"main\" routes the task to the MAIN CODER (the stronger \
         agentic engine) — use it for substantial, multi-file or build-and-verify work; \
         omit it (or \"mini\") for cheap mechanical edits.\n\n",
    );
    rules
}

// --- PLAN phase --------------------------------------------------------------

/// Build the PLAN prompt: goal + PRE-PLAN NOTES (the on-disk external memory) +
/// accumulated NOTES + the structure summary. It deliberately carries NO raw file
/// content — only the compact notes — so the single PLAN call stays small.
/// `prior_error`, when set, is the precise validation failure from the previous
/// attempt, prepended so the model can self-correct. `preplan_notes` is the
/// ALREADY-RENDERED (and hence already `MAX_PREPLAN_PROMPT_CHARS`-bounded)
/// [`crate::preplan::Preplan::render_for_prompt`] output; an empty string omits
/// the section entirely (a fresh planner with nothing yet remembered).
fn build_plan_prompt(
    goal: &str,
    preplan_notes: &str,
    notes_block: &str,
    summary: &serde_json::Value,
    prior_error: Option<&str>,
) -> String {
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
    // Defensive re-cap: `preplan_notes` is already bounded by the caller
    // (`render_for_prompt(MAX_PREPLAN_PROMPT_CHARS)`), but every other part of this
    // prompt re-caps its own input too — mirror that discipline rather than trust
    // a single upstream cap.
    let preplan_notes = truncate_chars(preplan_notes, MAX_PREPLAN_PROMPT_CHARS);
    if !preplan_notes.trim().is_empty() {
        prompt.push_str(
            "PRE-PLAN NOTES (your external memory from this planning session):\n",
        );
        prompt.push_str(&preplan_notes);
        prompt.push_str("\n\n");
    }
    prompt.push_str("FILE NOTES (from exploring the architectural spine):\n");
    prompt.push_str(notes_block);
    prompt.push_str("\n\nPROJECT SUMMARY: ");
    // The summary is raw, untrusted Oracle JSON and can be arbitrarily large; cap
    // it before appending.
    prompt.push_str(&truncate_chars(&summary.to_string(), MAX_SUMMARY_CHARS));
    prompt.push_str("\n\nRULES:\n");
    prompt.push_str(&task_rules_block(Some(MAX_TASKS)));
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

/// STRICT structural validation of a [`TasksPlan`] — the gate the planner runs before
/// SUBMIT and before the bulk Kanban create, so a malformed plan never reaches the human
/// gate or the board. (The server's `project_create_plan_tasks` independently re-validates
/// the DAG, but we reject early here with a precise, model-facing message so a bad plan is
/// caught at plan time, not at create time.) Enforces: non-empty
/// goal; ≥1 task; ≤ [`MAX_TASKS`]; per-task non-empty/bounded title + acceptance; `scope`
/// non-empty, ≤ [`MAX_TASK_SCOPE`], each a safe relative path; `contextFiles` ≤
/// [`MAX_TASK_CONTEXT`], each a safe relative path; unique non-empty ids; `dependsOn`
/// references EXISTING ids only (no self-dep, no dangling, no duplicates) and the graph
/// is ACYCLIC. It deliberately does NOT check the RUNTIME fields (`status`/`attempts`):
/// the runner mutates those, so only the plan-time wrapper requires `pending`/`0`.
pub(crate) fn validate_plan_structure(plan: &TasksPlan) -> Result<(), String> {
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
        // ROLE UNTANGLE Phase 4: weight is optional but, when present, must be a
        // known tier — a typo like "heavy" must be a retryable parse error, not a
        // silent mini fallback.
        if !matches!(task.weight.trim(), "" | "mini" | "main") {
            return Err(format!(
                "task {} has invalid weight {:?} (allowed: \"mini\" or \"main\")",
                task.id, task.weight
            ));
        }

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
            check_rel_path("scope entry", p).map_err(|e| format!("task `{}`: {e}", task.id))?;
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

/// Plan-time validation: structural validity ([`validate_plan_structure`]) PLUS the
/// freshness invariants that hold only at plan emission — every task starts `pending`
/// with a clean `attempts` counter. Those two runtime fields are the model's local
/// plan-draft state; the Kanban (not these fields) owns the real status once the tasks
/// are created. This stricter check stays on the PLAN phase so the model never submits a
/// plan whose draft tasks claim to be already-running.
fn validate_plan(plan: &TasksPlan) -> Result<(), String> {
    validate_plan_structure(plan)?;
    for task in &plan.tasks {
        if task.status != "pending" {
            return Err(format!(
                "task `{}` status must be \"pending\", got `{}`",
                task.id, task.status
            ));
        }
        // `attempts` is the model's local plan-draft counter: a freshly drafted task MUST
        // start at 0 (the runner tracks real attempts in-run; the board has no such field).
        if task.attempts != 0 {
            return Err(format!(
                "task `{}` attempts must be 0 at plan time, got {}",
                task.id, task.attempts
            ));
        }
    }
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
        plan.tasks.iter().all(|t| t
            .depends_on
            .iter()
            .all(|d| in_degree.contains_key(d.as_str()))),
        "detect_cycle precondition violated: a dependsOn references an unknown id \
         (validate_plan must run first)"
    );
    for task in &plan.tasks {
        for dep in &task.depends_on {
            dependents
                .entry(dep.as_str())
                .or_default()
                .push(task.id.as_str());
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
        return Err("the dependsOn graph has a cycle (it must be acyclic)".to_string());
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
            md.push_str(&format!(
                "- **Depends on:** {}\n",
                task.depends_on.join(", ")
            ));
        }
        md.push_str(&format!("- **Acceptance:** {}\n\n", task.acceptance.trim()));
    }
    md
}

// --- Kanban bulk-create payload ----------------------------------------------

/// Build the `tasks` array for `project_create_plan_tasks` from a validated
/// [`TasksPlan`]. Each entry carries the planner's INTERNAL id in `id`/`dependsOn`
/// (the server allocates fresh `T<n>` ids and REMAPS the deps); `scope`/`acceptance`
/// ride along so the runner's mini knows the write allowlist + the acceptance bar.
/// `contextFiles`/`status`/`attempts` are NOT sent: the board owns the runtime
/// status, and the read-only context files are not part of the 1a wire contract.
fn build_plan_tasks_payload(plan: &TasksPlan) -> serde_json::Value {
    let tasks: Vec<serde_json::Value> = plan
        .tasks
        .iter()
        .map(|t| {
            {
                let mut entry = serde_json::json!({
                    "id": t.id,
                    "title": t.title,
                    "scope": t.scope,
                    "acceptance": t.acceptance,
                    "dependsOn": t.depends_on,
                });
                // ROLE UNTANGLE Phase 4 NO-CHURN: only a "main"-weight task carries
                // the field on the wire; mini stays byte-identical to the 1a contract.
                if t.weight.trim() == "main" {
                    entry["weight"] = serde_json::json!("main");
                }
                entry
            }
        })
        .collect();
    serde_json::Value::Array(tasks)
}

/// Pull the `planId` out of the `project_create_plan_tasks` result so the outcome can
/// name where the plan landed. The tool echoes the `planId` we sent; a non-JSON or
/// planId-less body falls back to the `plan_id` we passed in (we know what we sent).
fn parse_created_plan_id(text: &str, fallback: &str) -> String {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| v.get("planId").and_then(|s| s.as_str()).map(str::to_string))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

// --- Hierarchical escalation (ADaPT pattern): OUTLINE + EXPAND + MERGE -------
//
// ADaPT ("As-Needed Decomposition and Planning for complex Tasks"): decompose ONLY
// when the flat plan fails, never pre-emptively. When [`run_planner`]'s flat PLAN
// loop exhausts [`FLAT_PLAN_ATTEMPTS`], it escalates here instead of a doomed third
// identical flat try:
//   1. OUTLINE — one model call decomposes the goal into a small DAG of milestones.
//   2. EXPAND — one SEQUENTIAL model call PER milestone (no concurrency: the local
//      model endpoint has no throttling), each producing a small flat [`TasksPlan`]
//      fragment scoped to JUST that milestone.
//   3. MERGE ([`merge_fragments`]) — PURE CODE, no model call: namespaces every
//      fragment's task ids, synthesizes cross-milestone ordering from the milestone
//      DAG (never asking the weak model to reason about another fragment's tasks),
//      then re-runs the EXISTING flat-plan validation on the assembled whole.

/// One milestone the OUTLINE stage decomposes the goal into. `files` are ADVISORY
/// scope hints fed into the EXPAND prompt — NOT an enforced task scope (that
/// enforcement lives on the per-task `scope` each EXPAND fragment itself emits, via
/// the SAME [`validate_plan_structure`] the flat path uses). `id`/`dependsOn` wire
/// the milestone DAG [`merge_fragments`] uses to synthesize cross-fragment task
/// ordering — the weak local model never has to reason about another milestone's
/// tasks directly.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Milestone {
    /// Unique, non-empty id (e.g. `M1`) — the namespace prefix
    /// [`merge_fragments`] gives every task pulled from this milestone's fragment
    /// (`"{id}-{taskId}"`, e.g. `M1-T001`).
    id: String,
    /// One-line title, fed into the EXPAND prompt as this milestone's framing.
    title: String,
    /// Advisory file scope hints (project-root-relative); may be empty. Any entry
    /// that fails [`check_rel_path`] is silently dropped by [`parse_outline`] — an
    /// advisory hint, unlike a task `scope` path, is never worth failing the whole
    /// outline over.
    #[serde(default)]
    files: Vec<String>,
    /// Ids of prerequisite milestones. Must reference EXISTING milestone ids; the
    /// whole graph must be acyclic — both checked by [`validate_outline`].
    #[serde(default)]
    depends_on: Vec<String>,
}

/// The OUTLINE stage's single model output: the milestone decomposition.
/// `deny_unknown_fields`: mirrors [`TasksPlan`]'s discipline — a typo'd/extra key is
/// a hard parse error fed back as a retry message, not silently ignored.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Outline {
    milestones: Vec<Milestone>,
}

/// Reject a cyclic milestone `dependsOn` graph. A small mirror of [`detect_cycle`]'s
/// Kahn's-algorithm approach over [`Milestone`] instead of [`Task`] — kept as a
/// SEPARATE copy (not a shared generic) so the flat path's `detect_cycle` stays
/// completely untouched (byte-stable happy path) while this new, independent stage
/// gets its own tiny, easy-to-audit cycle check.
///
/// PRECONDITION (mirrors [`detect_cycle`]'s own): must run AFTER the dangling-dep
/// pass in [`validate_outline`], which has already rejected a `dependsOn` entry that
/// does not reference an existing milestone id.
fn detect_milestone_cycle(outline: &Outline) -> Result<(), String> {
    let mut in_degree: HashMap<&str, usize> = HashMap::with_capacity(outline.milestones.len());
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::with_capacity(outline.milestones.len());
    for m in &outline.milestones {
        in_degree.insert(m.id.as_str(), m.depends_on.len());
    }
    debug_assert!(
        outline.milestones.iter().all(|m| m
            .depends_on
            .iter()
            .all(|d| in_degree.contains_key(d.as_str()))),
        "detect_milestone_cycle precondition violated: a dependsOn references an unknown \
         milestone id (validate_outline must run first)"
    );
    for m in &outline.milestones {
        for dep in &m.depends_on {
            dependents.entry(dep.as_str()).or_default().push(m.id.as_str());
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
    if resolved != outline.milestones.len() {
        return Err("the milestone dependsOn graph has a cycle (it must be acyclic)".to_string());
    }
    Ok(())
}

/// Structural validation of an [`Outline`]: non-empty; ≤ [`MAX_MILESTONES`]; unique
/// non-empty ids; non-empty bounded titles; `dependsOn` references EXISTING
/// milestone ids only (no self-dep, no dangling, no duplicates) and is acyclic.
/// Mirrors [`validate_plan_structure`]'s discipline at the milestone-DAG level
/// rather than the task-DAG level.
fn validate_outline(outline: &Outline) -> Result<(), String> {
    if outline.milestones.is_empty() {
        return Err("outline has no milestones".to_string());
    }
    if outline.milestones.len() > MAX_MILESTONES {
        return Err(format!(
            "too many milestones: {} (max {MAX_MILESTONES})",
            outline.milestones.len()
        ));
    }
    let mut ids: HashSet<&str> = HashSet::with_capacity(outline.milestones.len());
    for m in &outline.milestones {
        if m.id.trim().is_empty() {
            return Err("a milestone has an empty `id`".to_string());
        }
        if !ids.insert(m.id.as_str()) {
            return Err(format!("duplicate milestone id `{}`", m.id));
        }
        check_field("milestone title", &m.title)?;
    }
    for m in &outline.milestones {
        let mut seen_deps: HashSet<&str> = HashSet::with_capacity(m.depends_on.len());
        for dep in &m.depends_on {
            if dep == &m.id {
                return Err(format!("milestone `{}` depends on itself", m.id));
            }
            if !ids.contains(dep.as_str()) {
                return Err(format!(
                    "milestone `{}` dependsOn references unknown milestone id `{dep}`",
                    m.id
                ));
            }
            if !seen_deps.insert(dep.as_str()) {
                return Err(format!(
                    "milestone `{}` has a duplicate dependsOn entry `{dep}`",
                    m.id
                ));
            }
        }
    }
    detect_milestone_cycle(outline)?;
    Ok(())
}

/// Parse + validate the OUTLINE stage's single fenced ```outline``` block. `files`
/// entries are advisory scope hints: any that fail [`check_rel_path`] are silently
/// dropped (never fail the whole outline over an advisory hint) BEFORE structural
/// validation runs.
fn parse_outline(raw: &str) -> Result<Outline, String> {
    let body = extract_one_block(raw, "outline")?;
    let mut outline: Outline =
        serde_json::from_str(body).map_err(|e| format!("outline JSON invalid: {e}"))?;
    for m in outline.milestones.iter_mut() {
        m.files
            .retain(|f| check_rel_path("milestone files entry", f).is_ok());
    }
    validate_outline(&outline)?;
    Ok(outline)
}

/// Build the OUTLINE prompt: goal + PRE-PLAN NOTES + structure summary + the flat
/// path's two failure reasons (so the model understands WHY it is being asked to
/// decompose instead of try the same flat ask a third time) + the outline schema.
/// `prior_error`, when set, is THIS stage's own previous (rejected) outline attempt
/// — distinct from `flat_failures`, which is always the flat-PLAN failure context.
/// Hard-capped at [`MAX_PLAN_PROMPT_CHARS`], mirroring [`build_plan_prompt`].
fn build_outline_prompt(
    goal: &str,
    preplan_notes: &str,
    summary: &serde_json::Value,
    flat_failures: &str,
    prior_error: Option<&str>,
) -> String {
    let mut prompt = String::new();
    if let Some(err) = prior_error {
        let err = truncate_chars(err, MAX_PRIOR_ERROR_CHARS);
        prompt.push_str(&format!(
            "YOUR PREVIOUS OUTLINE WAS REJECTED: {err}\nFix it and emit a corrected outline.\n\n"
        ));
    }
    prompt.push_str(
        "You are the PLANNER. Produce a milestone OUTLINE — a small DAG of milestones — before \
         planning each in detail.\n\n",
    );
    let flat_failures = truncate_chars(flat_failures, MAX_PRIOR_ERROR_CHARS);
    prompt.push_str(&format!(
        "The flat plan failed twice: {flat_failures}; now decompose the goal into milestones.\n\n"
    ));
    let goal_capped = truncate_chars(goal, MAX_GOAL_CHARS);
    prompt.push_str(&format!("GOAL: {goal_capped}\n\n"));
    let preplan_notes = truncate_chars(preplan_notes, MAX_PREPLAN_PROMPT_CHARS);
    if !preplan_notes.trim().is_empty() {
        prompt.push_str("PRE-PLAN NOTES (your external memory from this planning session):\n");
        prompt.push_str(&preplan_notes);
        prompt.push_str("\n\n");
    }
    prompt.push_str("PROJECT SUMMARY: ");
    prompt.push_str(&truncate_chars(&summary.to_string(), MAX_SUMMARY_CHARS));
    prompt.push_str("\n\nRULES:\n");
    prompt.push_str(&format!("- At most {MAX_MILESTONES} milestones.\n"));
    prompt.push_str("- Each milestone `id` is unique and non-empty (e.g. M1).\n");
    prompt.push_str(
        "- `dependsOn` lists prerequisite milestone ids (existing ids only) and MUST be acyclic.\n",
    );
    prompt.push_str(
        "- `files` are advisory scope hints (project-root-relative paths); may be empty.\n\n",
    );
    prompt.push_str(
        "Emit EXACTLY ONE fenced ```outline``` block — a single JSON object — and nothing else:\n\
         ```outline\n\
         {\"milestones\": [\n\
         {\"id\": \"M1\", \"title\": \"...\", \"files\": [\"path/a\"], \"dependsOn\": []}\n\
         ]}\n\
         ```",
    );
    // Final guard, mirroring build_plan_prompt / build_explore_prompt.
    truncate_chars(&prompt, MAX_PLAN_PROMPT_CHARS)
}

/// Build the EXPAND prompt for ONE milestone: goal + PRE-PLAN NOTES + this
/// milestone's title/advisory files + the SAME [`task_rules_block`] the flat PLAN
/// prompt uses (so the two schemas cannot drift) + the fragment-local-id rule +
/// the same `plan` schema [`build_plan_prompt`] asks for. Hard-capped at
/// [`MAX_PLAN_PROMPT_CHARS`]. Deliberately carries NO other milestone's tasks —
/// each EXPAND call is fresh and small, exactly like an EXPLORE call.
fn build_expand_prompt(
    goal: &str,
    preplan_notes: &str,
    milestone: &Milestone,
    prior_error: Option<&str>,
) -> String {
    let mut prompt = String::new();
    if let Some(err) = prior_error {
        let err = truncate_chars(err, MAX_PRIOR_ERROR_CHARS);
        prompt.push_str(&format!(
            "YOUR PREVIOUS FRAGMENT PLAN WAS REJECTED: {err}\nFix it and emit a corrected plan.\n\n"
        ));
    }
    prompt.push_str(
        "You are the PLANNER. Produce an ATOMIC implementation plan (a DAG of small tasks) for \
         ONE MILESTONE only — not the whole goal.\n\n",
    );
    let goal_capped = truncate_chars(goal, MAX_GOAL_CHARS);
    prompt.push_str(&format!("OVERALL GOAL: {goal_capped}\n\n"));
    let preplan_notes = truncate_chars(preplan_notes, MAX_PREPLAN_PROMPT_CHARS);
    if !preplan_notes.trim().is_empty() {
        prompt.push_str("PRE-PLAN NOTES (your external memory from this planning session):\n");
        prompt.push_str(&preplan_notes);
        prompt.push_str("\n\n");
    }
    prompt.push_str(&format!(
        "MILESTONE {}: {}\n",
        milestone.id,
        milestone.title.trim()
    ));
    if milestone.files.is_empty() {
        prompt.push_str("Advisory file scope hints: (none suggested)\n");
    } else {
        prompt.push_str(&format!(
            "Advisory file scope hints: {}\n",
            milestone.files.join(", ")
        ));
    }
    prompt.push_str("\nRULES:\n");
    prompt.push_str(&task_rules_block(None));
    prompt.push_str(
        "- Task ids in THIS fragment are LOCAL: `dependsOn` may ONLY reference an id defined in \
         THIS SAME fragment — never a task id from another milestone.\n\n",
    );
    prompt.push_str(
        "Emit EXACTLY ONE fenced ```plan``` block — a single JSON object — and nothing else:\n\
         ```plan\n\
         {\"projectGoal\": \"...\", \"tasks\": [\n\
         {\"id\": \"T001\", \"title\": \"...\", \"scope\": [\"path/a\"], \"contextFiles\": [], \
         \"acceptance\": \"cargo test passes\", \"dependsOn\": [], \"status\": \"pending\", \"attempts\": 0}\n\
         ]}\n\
         ```",
    );
    truncate_chars(&prompt, MAX_PLAN_PROMPT_CHARS)
}

/// Parse + validate ONE EXPAND fragment for `milestone_id`: reuses the EXACT flat-PLAN
/// parse (a single fenced ```plan``` block, the SAME [`TasksPlan`] shape) and the EXACT
/// [`validate_plan_structure`] structural gate the flat path uses — deliberately NOT
/// [`validate_plan`] (no freshness re-check HERE; [`merge_fragments`]'s Pass 1
/// unconditionally NORMALIZES every task's `status`/`attempts` back to
/// `"pending"`/`0` regardless of what this fragment carries, so re-validating
/// freshness at parse time would be redundant — the invariant holds by
/// construction downstream, not by rejecting a bad echo here). Because
/// `validate_plan_structure` only ever considers ids WITHIN `plan.tasks`, it ALREADY
/// enforces "dependsOn is local to the fragment" for free — a dep naming a task from
/// another milestone is simply an unknown id from this fragment's point of view.
fn parse_expand_fragment(raw: &str, milestone_id: &str) -> Result<TasksPlan, String> {
    let body = extract_one_block(raw, "plan")?;
    let parsed: TasksPlan =
        serde_json::from_str(body).map_err(|e| format!("fragment plan JSON invalid: {e}"))?;
    validate_plan_structure(&parsed)
        .map_err(|e| format!("milestone {milestone_id} fragment invalid: {e}"))?;
    Ok(parsed)
}

/// Task ids (LOCAL, un-namespaced) in `tasks` with an EMPTY `dependsOn` — the entry
/// points of this fragment's internal DAG. [`merge_fragments`] wires these to the
/// prerequisite milestone's leaves.
fn fragment_roots(tasks: &[Task]) -> Vec<&str> {
    tasks
        .iter()
        .filter(|t| t.depends_on.is_empty())
        .map(|t| t.id.as_str())
        .collect()
}

/// Task ids (LOCAL, un-namespaced) in `tasks` that NO OTHER task in the same
/// fragment depends on — the exit points of this fragment's internal DAG.
/// [`merge_fragments`] wires these into the dependent milestone's roots.
fn fragment_leaves(tasks: &[Task]) -> Vec<&str> {
    let depended_on: HashSet<&str> = tasks
        .iter()
        .flat_map(|t| t.depends_on.iter().map(|d| d.as_str()))
        .collect();
    tasks
        .iter()
        .filter(|t| !depended_on.contains(t.id.as_str()))
        .map(|t| t.id.as_str())
        .collect()
}

/// MERGE: the pure-code heart of the hierarchical escalation path. `fragments` MUST
/// be in the SAME order as `outline.milestones` (index `i` is milestone `i`'s
/// EXPAND output) — [`run_hierarchical_plan`] guarantees this (EXPAND runs
/// sequentially in outline order and pushes each result in turn).
///
/// Deterministic rule, in three passes:
/// 1. NAMESPACE every fragment task id to `"{milestoneId}-{taskId}"` (e.g.
///    `M1-T001`) and remap its `dependsOn` the same way. A `dependsOn` entry that
///    does not reference a task id WITHIN THE SAME fragment is a hard error — ids
///    are local to the fragment; a model that drifted outside it is rejected here
///    (this is checked independently of any upstream per-fragment validation, so
///    this function is safe to call, and to unit-test, standalone). This same pass
///    also NORMALIZES the runtime fields: every namespaced task's `status` is
///    forced to `"pending"` and `attempts` to `0`, UNCONDITIONALLY overwriting
///    whatever the EXPAND fragment echoed. The harness owns these two fields, never
///    the model; an EXPAND call that hallucinated (or echoed from training data) a
///    `"done"`/`attempts > 0` task must never poison the merged plan with a task
///    that looks already-finished.
/// 2. Cross-fragment ORDERING, from the milestone DAG alone (never asking the model
///    to reason across fragments): for every milestone edge `Ma -> Mb` (`Mb`
///    `dependsOn` `Ma`), every ROOT task of `Mb`'s fragment gets `dependsOn` +=
///    ALL LEAF tasks of `Ma`'s fragment ([`fragment_roots`] / [`fragment_leaves`]).
/// 3. ASSEMBLE: `projectGoal` is the OUTER `goal` (capped like the flat path caps
///    its own goal in the PLAN prompt — see [`MAX_GOAL_CHARS`]). Every task's
///    `status`/`attempts` are ALREADY `"pending"`/`0` (Pass 1's normalization
///    guarantees this unconditionally, for every task, before this pass ever
///    runs) — so the flat path's freshness check, `validate_plan`, is intentionally
///    NOT re-run here; only structure/cycles, per the merge contract, because the
///    freshness invariant it would check is already guaranteed by construction.
///    Then the EXISTING [`validate_plan_structure`] (which itself runs
///    [`detect_cycle`]) is run on the WHOLE assembled plan: global id uniqueness
///    (guaranteed by namespacing, but checked anyway), dangling deps, cycles,
///    [`MAX_TASKS`], and per-task `scope` caps. A merge that produced more than
///    [`MAX_TASKS`] tasks gets that validator error WRAPPED with an explicit hint
///    naming the overflow.
fn merge_fragments(goal: &str, outline: &Outline, fragments: &[TasksPlan]) -> Result<TasksPlan, String> {
    if outline.milestones.len() != fragments.len() {
        return Err(format!(
            "hierarchical merge: {} milestone(s) but {} fragment(s) (must match 1:1)",
            outline.milestones.len(),
            fragments.len()
        ));
    }

    // Pass 1: per-milestone namespace remap + local-id (dangling-dep) validation +
    // runtime-field normalization (status/attempts forced to pending/0 below).
    // `remapped[i]` holds milestone `i`'s tasks, namespaced, in original order.
    let mut remapped: Vec<Vec<Task>> = Vec::with_capacity(fragments.len());
    let mut roots_by_milestone: HashMap<&str, Vec<String>> = HashMap::with_capacity(outline.milestones.len());
    let mut leaves_by_milestone: HashMap<&str, Vec<String>> = HashMap::with_capacity(outline.milestones.len());

    for (milestone, fragment) in outline.milestones.iter().zip(fragments.iter()) {
        let mid = milestone.id.as_str();
        let local_ids: HashSet<&str> = fragment.tasks.iter().map(|t| t.id.as_str()).collect();
        let root_local: HashSet<&str> = fragment_roots(&fragment.tasks).into_iter().collect();
        let leaf_local: HashSet<&str> = fragment_leaves(&fragment.tasks).into_iter().collect();

        let mut namespaced_tasks = Vec::with_capacity(fragment.tasks.len());
        let mut roots = Vec::new();
        let mut leaves = Vec::new();
        for task in &fragment.tasks {
            let namespaced_id = format!("{mid}-{}", task.id);
            let mut namespaced_deps = Vec::with_capacity(task.depends_on.len());
            for dep in &task.depends_on {
                if !local_ids.contains(dep.as_str()) {
                    return Err(format!(
                        "milestone {mid}: task `{}` dependsOn `{dep}`, which is not a task in \
                         this fragment (task ids are local to the fragment)",
                        task.id
                    ));
                }
                namespaced_deps.push(format!("{mid}-{dep}"));
            }
            if root_local.contains(task.id.as_str()) {
                roots.push(namespaced_id.clone());
            }
            if leaf_local.contains(task.id.as_str()) {
                leaves.push(namespaced_id.clone());
            }
            let mut namespaced_task = task.clone();
            namespaced_task.id = namespaced_id;
            namespaced_task.depends_on = namespaced_deps;
            // The harness OWNS runtime fields, never the model: force every
            // namespaced task back to a fresh draft state regardless of what the
            // EXPAND fragment echoed. A model that hallucinated (or copy-pasted
            // from training data) a "done"/`attempts > 0` task must never poison
            // the merged plan with a task that looks already-finished — this is
            // the SAME freshness invariant `validate_plan` enforces on the flat
            // path, applied here at the SOURCE (Pass 1) rather than re-validated
            // at the end, so it holds unconditionally, not just when re-checked.
            namespaced_task.status = "pending".to_string();
            namespaced_task.attempts = 0;
            namespaced_tasks.push(namespaced_task);
        }
        roots_by_milestone.insert(mid, roots);
        leaves_by_milestone.insert(mid, leaves);
        remapped.push(namespaced_tasks);
    }

    // Pass 2: cross-fragment ordering via the milestone DAG.
    for (i, milestone) in outline.milestones.iter().enumerate() {
        let mut extra_deps: Vec<String> = Vec::new();
        for prereq in &milestone.depends_on {
            if let Some(leaves) = leaves_by_milestone.get(prereq.as_str()) {
                extra_deps.extend(leaves.iter().cloned());
            }
        }
        if extra_deps.is_empty() {
            continue;
        }
        let roots = roots_by_milestone
            .get(milestone.id.as_str())
            .cloned()
            .unwrap_or_default();
        for task in remapped[i].iter_mut() {
            if roots.contains(&task.id) {
                for dep in &extra_deps {
                    if !task.depends_on.contains(dep) {
                        task.depends_on.push(dep.clone());
                    }
                }
            }
        }
    }

    // Pass 3: assemble + validate the whole.
    let mut all_tasks: Vec<Task> = Vec::new();
    for tasks in remapped {
        all_tasks.extend(tasks);
    }
    let merged = TasksPlan {
        project_goal: truncate_chars(goal, MAX_GOAL_CHARS),
        tasks: all_tasks,
    };
    let task_count = merged.tasks.len();
    validate_plan_structure(&merged).map_err(|e| {
        if task_count > MAX_TASKS {
            format!("hierarchical merge produced {task_count}>{MAX_TASKS} tasks: {e}")
        } else {
            e
        }
    })?;
    Ok(merged)
}

/// Drive the whole hierarchical escalation path: OUTLINE (bounded retries) -> EXPAND
/// (sequential, one bounded-retry model call PER milestone, NO concurrency — the
/// local model endpoint has no throttling) -> MERGE (pure code). Called by
/// [`run_planner`] ONLY after BOTH flat PLAN attempts have failed; `flat_failure_reasons`
/// is fed into the OUTLINE prompt so the model understands why it is decomposing
/// instead of repeating the same flat ask. `preplan_notes` is the SAME already-rendered
/// snapshot [`run_planner`] captured once before its flat retry loop (this stage does
/// not re-render it — the accepted outline is threaded directly into each EXPAND
/// prompt via `milestone`, not round-tripped through the preplan file).
///
/// Returns `Err` naming whichever stage failed (OUTLINE exhaustion, a specific
/// milestone's EXPAND exhaustion, or MERGE) — surfaced by [`run_planner`] to the
/// burst model exactly like today's flat-exhaustion error.
async fn run_hierarchical_plan(
    goal: &str,
    model: &dyn CoderModel,
    preplan: &Preplan,
    preplan_notes: &str,
    summary: &serde_json::Value,
    flat_failure_reasons: &str,
    activity: &Activity,
) -> Result<TasksPlan, String> {
    // --- OUTLINE (bounded retries) ---
    activity.milestone("outlining plan (hierarchical)", Node::Hollow);
    let mut outline_error: Option<String> = None;
    let mut outline: Option<Outline> = None;
    for _ in 0..MAX_OUTLINE_ATTEMPTS {
        let prompt = build_outline_prompt(
            goal,
            preplan_notes,
            summary,
            flat_failure_reasons,
            outline_error.as_deref(),
        );
        let transcript = Transcript::new(prompt);
        let raw = model.next_output(&transcript).await;
        match parse_outline(&raw) {
            Ok(o) => {
                outline = Some(o);
                break;
            }
            Err(e) => outline_error = Some(e),
        }
    }
    let outline = outline.ok_or_else(|| {
        format!(
            "hierarchical OUTLINE failed after {MAX_OUTLINE_ATTEMPTS} attempt(s): {}",
            outline_error.unwrap_or_else(|| "unknown error".to_string())
        )
    })?;

    // Persist the ACCEPTED outline to the external memory: one line per milestone.
    for m in &outline.milestones {
        let deps = if m.depends_on.is_empty() {
            "none".to_string()
        } else {
            m.depends_on.join(", ")
        };
        preplan.append(
            "Draft outline",
            &format!("- {}: {} (depends on: {deps})", m.id, m.title.trim()),
        );
    }

    // --- EXPAND: sequential, one bounded-retry model call PER milestone ---
    let n = outline.milestones.len();
    let mut fragments: Vec<TasksPlan> = Vec::with_capacity(n);
    for (i, milestone) in outline.milestones.iter().enumerate() {
        activity.milestone(
            &format!("expanding milestone {} ({}/{n})", milestone.id, i + 1),
            Node::Hollow,
        );
        let mut fragment_error: Option<String> = None;
        let mut fragment: Option<TasksPlan> = None;
        for _ in 0..MAX_EXPAND_ATTEMPTS {
            let prompt =
                build_expand_prompt(goal, preplan_notes, milestone, fragment_error.as_deref());
            let transcript = Transcript::new(prompt);
            let raw = model.next_output(&transcript).await;
            match parse_expand_fragment(&raw, &milestone.id) {
                Ok(p) => {
                    fragment = Some(p);
                    break;
                }
                Err(e) => fragment_error = Some(e),
            }
        }
        let fragment = fragment.ok_or_else(|| {
            format!(
                "hierarchical EXPAND for milestone {} failed after {MAX_EXPAND_ATTEMPTS} attempt(s): {}",
                milestone.id,
                fragment_error.unwrap_or_else(|| "unknown error".to_string())
            )
        })?;
        fragments.push(fragment);
    }

    // --- MERGE (pure code, no model call) ---
    merge_fragments(goal, &outline, &fragments)
}

// --- The routine -------------------------------------------------------------

/// Run the local planner for `goal`: STRUCTURE -> EXPLORE -> PLAN -> SUBMIT ->
/// (on approval) create the tasks on the Kanban.
///
/// Drives the injected `model` (small local LLM) + `mcp` (Oracle server) + `fs`
/// (root-confined reads). `project_id` is the Oracle-side project key the
/// `project_structure` / `plan_submit` / `project_create_plan_tasks` tools require.
/// Returns a [`PlanOutcome`] on success, or an Escalated-style error string the caller
/// surfaces to the burst model (e.g. the model never produced a valid plan within the
/// retry budget, the structure tool failed, `plan_submit` errored, or the bulk Kanban
/// create failed after an approval).
pub async fn run_planner(
    goal: &str,
    model: &dyn CoderModel,
    mcp: &dyn McpBackend,
    fs: &FsBackend,
    project_id: &str,
    activity: &Activity,
    // Orchestrator-composer auto-create toggle: when false, an APPROVED plan is NOT turned into
    // board tasks (the operator creates them); when true (default), tasks are created on approval.
    auto_create: bool,
) -> Result<PlanOutcome, String> {
    let goal = goal.trim();
    if goal.is_empty() {
        return Err("planner needs a non-empty goal".to_string());
    }
    if project_id.trim().is_empty() {
        return Err("planner needs a project_id (DEVBOULE_PROJECT_ID not set?)".to_string());
    }

    // --- 0) PRE-PLAN NOTES: harness-owned on-disk external memory ---
    // Rooted at `<fs.root>/.devboule/preplan.md`. Resumes when a prior (possibly
    // crashed) run's file belongs to this SAME goal — so a planner that dies mid-run
    // leaves its findings for the next attempt to re-read instead of re-discovering
    // everything from scratch; a DIFFERENT goal never inherits stale memory.
    let preplan = Preplan::load_or_init(fs.root(), goal);

    // --- 1) STRUCTURE (no LLM) ---
    let structure_text = mcp
        .call_tool(
            "project_structure",
            serde_json::json!({ "project_id": project_id }),
        )
        .await
        .map_err(|e| format!("project_structure failed: {e}"))?;
    let structure = parse_structure(&structure_text)?;
    // STRUCTURE done → a coarse coder-tier milestone the Console shows live: how many
    // spine files we are about to explore (a label only — no paths, no bodies).
    activity.milestone(
        &format!("Planning: {} spine files", structure.spine.len()),
        Node::Dot,
    );
    preplan.append(
        "Findings",
        &format!(
            "- STRUCTURE spine: {}",
            structure
                .spine
                .iter()
                .map(|e| e.path.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    );

    // --- 1b) GOAL-driven grounding merged into the spine (best-effort) ---
    // A weak local model cannot reliably guess which INFRA files matter for THIS
    // goal from the structural spine alone; ground it with whatever the Oracle's
    // semantic index surfaces for the goal itself, on top of (never instead of)
    // the purely-structural top-MAX_SPINE files. Any failure here — a tool error,
    // unparseable JSON, an all-unsafe/all-duplicate chunk set — degrades to the
    // structural spine alone; it never sinks the plan.
    activity.milestone("grounding goal via oracle", Node::Hollow);
    let mut explore_spine: Vec<SpineEntry> = structure.spine.clone();
    let structural_paths: HashSet<String> =
        explore_spine.iter().map(|e| e.path.clone()).collect();
    if let Ok(goal_ctx_raw) = mcp
        .call_tool(
            "oracle_context",
            serde_json::json!({ "query": goal, "limit": 8, "project_id": project_id }),
        )
        .await
    {
        if let Some(parsed) = parse_oracle_context(&goal_ctx_raw) {
            explore_spine.extend(goal_spine_entries(&parsed, &structural_paths));
        }
    }

    // --- 2) EXPLORE (bounded LLM loop, ONE small call per spine file) ---
    // We store the ALREADY-RENDERED note line (not the parsed note) so the budget
    // accounting and the final join use the exact same string — `render_note` is
    // called once per accepted note, never twice.
    let mut rendered_notes: Vec<String> = Vec::with_capacity(explore_spine.len());
    let mut notes_total = 0usize;
    for entry in &explore_spine {
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

        // EXPLORE milestone — one per spine file we ACTUALLY explore (a read that
        // failed `continue`d above, so a skipped file emits nothing, matching the
        // "one model call per explored file" invariant). The basename keeps the
        // label short + privacy-safe; node "" (hollow) marks an in-progress step.
        activity.milestone(
            &format!("exploring {}", path_basename(&entry.path)),
            Node::Hollow,
        );

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
            .map(|raw| build_grounding_text(&raw, MAX_GROUNDING_CHARS));

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
                // Persist EVERY accepted note to the external memory, regardless of
                // whether it also makes THIS run's in-prompt notes budget below —
                // the on-disk record outlives a single PLAN call's char cap.
                preplan.append("Findings", &rendered);
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

    // --- 3) PLAN: FLAT attempts first, ESCALATE to hierarchical on failure ---
    // Rendered ONCE, before the retry loop: by this point STRUCTURE + every
    // accepted EXPLORE note is already on disk, so even attempt #1 carries the
    // full external-memory context a crash-resumed run needs. Also reused, WITHOUT
    // re-rendering, by the hierarchical escalation path below (same discipline).
    let preplan_notes = preplan.render_for_prompt(MAX_PREPLAN_PROMPT_CHARS);
    // ADaPT pattern (escalation-ONLY, failure-driven): a SIMPLE goal that succeeds on
    // attempt 1 never touches anything below this loop — no size-threshold
    // pre-escalation exists. Only when BOTH flat attempts fail does the code below
    // the loop run the hierarchical OUTLINE/EXPAND/MERGE path in place of what would
    // have been a third, identical flat try.
    let mut flat_errors: Vec<String> = Vec::new();
    let mut plan: Option<TasksPlan> = None;
    for attempt in 0..FLAT_PLAN_ATTEMPTS {
        let prompt = build_plan_prompt(
            goal,
            &preplan_notes,
            &notes_block,
            &structure.summary,
            flat_errors.last().map(|s| s.as_str()),
        );
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
            Err(e) => {
                preplan.append(
                    "Decisions",
                    &format!("attempt {} rejected: {e}", attempt + 1),
                );
                flat_errors.push(e);
            }
        }
    }
    let plan = match plan {
        Some(p) => p,
        None => {
            let flat_failure_reasons = flat_errors.join("; ");
            run_hierarchical_plan(
                goal,
                model,
                &preplan,
                &preplan_notes,
                &structure.summary,
                &flat_failure_reasons,
                activity,
            )
            .await
            .map_err(|e| format!("planner could not produce a valid plan: {e}"))?
        }
    };
    // PLAN done → how many tasks were drafted (a count label only). Shared by BOTH
    // the flat and hierarchical paths — everything from here down is IDENTICAL
    // regardless of which path produced `plan`, INCLUDING the freshness invariant
    // every downstream consumer relies on: every task's `status` is `"pending"` and
    // `attempts` is `0`. The flat path gets this from `validate_plan` (above); the
    // hierarchical path gets it from `merge_fragments`'s Pass 1 normalization
    // (unconditional, not a re-validation) — so this milestone, and everything
    // after it, can treat `plan` the same way no matter which path built it.
    activity.milestone(&format!("drafted {} tasks", plan.tasks.len()), Node::Dot);

    // --- 4) SUBMIT: plan_submit (human gate). Nothing is persisted locally and
    // NOTHING is created on the board yet — an unapproved/rejected plan must never
    // pollute the Kanban, so the bulk create happens ONLY after an `approved` verdict.
    let plan_markdown = render_plan_markdown(&plan);
    let title = format!(
        "Devboule plan: {}",
        truncate_chars(goal, 120).replace('\n', " ")
    );
    // SUBMIT → the plan is now in front of the human gate. `plan_submit` BLOCKS for
    // approval, so this milestone is the live "waiting on you" signal in the Console.
    // Node terra = the warm/awaiting ring (the wire contract has no "coral" node).
    activity.milestone("plan submitted — awaiting approval", Node::Terra);
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

    // The gate carries BOTH the human verdict (`status`) and the `planId` the plan was
    // filed under — we need the planId to tag the tasks we create on the board.
    let SubmitResult { approval, plan_id } = parse_submit_result(&submit_text);
    // Result milestone: approved (sage), rejected/timeout (terra — the warm ring, as
    // the contract has no coral node; the TEXT carries the rejected/timed-out meaning).
    let (result_text, result_node) = match approval {
        PlanApproval::Approved => ("plan approved", Node::Sage),
        PlanApproval::Rejected => ("plan rejected", Node::Terra),
        PlanApproval::Timeout => ("plan approval timed out", Node::Terra),
    };
    activity.milestone(result_text, result_node);

    // --- 5) CREATE ON THE BOARD (approved ONLY) ---
    // On any non-approved verdict we stop here: nothing is created. On `approved` we
    // bulk-create the plan's tasks on the Kanban via `project_create_plan_tasks`,
    // passing the planner's INTERNAL ids in id/dependsOn (the server allocates fresh
    // T<n> ids + remaps the deps) tagged with the approved `planId`. A missing planId
    // here is a server-contract violation (an approval must carry one): surface it as a
    // hard error rather than create un-tagged tasks the runner could never find.
    let created_plan_id = if approval == PlanApproval::Approved && auto_create {
        let plan_id = plan_id.ok_or_else(|| {
            "plan_submit approved the plan but returned no planId; cannot create tasks on the board"
                .to_string()
        })?;
        let create_text = mcp
            .call_tool(
                "project_create_plan_tasks",
                serde_json::json!({
                    "project_id": project_id,
                    "plan_id": plan_id,
                    "tasks": build_plan_tasks_payload(&plan),
                }),
            )
            .await
            .map_err(|e| format!("project_create_plan_tasks failed: {e}"))?;
        let created_plan_id = parse_created_plan_id(&create_text, &plan_id);
        activity.milestone(
            &format!("{} task(s) created on the board", plan.tasks.len()),
            Node::Sage,
        );
        Some(created_plan_id)
    } else {
        if approval == PlanApproval::Approved {
            // auto-create OFF: the plan IS approved, but its tasks are left for the operator to
            // create (the composer's "auto-create: off"). Note it so the run is not silent.
            activity.milestone(
                &format!(
                    "plan approved; {} task(s) NOT auto-created (auto-create off)",
                    plan.tasks.len()
                ),
                Node::Sage,
            );
        }
        None
    };

    // A TERMINAL human verdict (approved or rejected) ends this planning session —
    // clear the external memory so it never leaks into a later, unrelated run. A
    // `Timeout` is NOT terminal (the human simply did not answer in time): leave the
    // file so a retry with the SAME goal still resumes with everything gathered.
    if matches!(approval, PlanApproval::Approved | PlanApproval::Rejected) {
        preplan.clear();
    }

    Ok(PlanOutcome {
        tasks_plan: plan,
        approval,
        plan_id: created_plan_id,
    })
}

/// The two things the `plan_submit` gate returns: the human verdict + the server's
/// `planId` for the submitted plan.
struct SubmitResult {
    approval: PlanApproval,
    /// The `planId` the server filed the plan under, when present. `None` for a
    /// non-JSON / planId-less body (the approval path then hard-errors — see
    /// `run_planner` step 5 — since an approval MUST carry a planId).
    plan_id: Option<String>,
}

/// Parse the `plan_submit` result text into a [`SubmitResult`]. The tool returns a JSON
/// object `{planId, status}`; a non-JSON or status-less body maps to the conservative
/// `Timeout` (never a false `Approved`) with no planId.
fn parse_submit_result(text: &str) -> SubmitResult {
    let value = serde_json::from_str::<serde_json::Value>(text).ok();
    let plan_id = value
        .as_ref()
        .and_then(|v| v.get("planId").and_then(|s| s.as_str()).map(str::to_string))
        // P2: trim BEFORE filtering so a whitespace-only planId (e.g. " ") is treated
        // as absent — not sent to `project_create_plan_tasks` as a blank tag.
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let approval = value.as_ref().and_then(|v| {
        v.get("status")
            .and_then(|s| s.as_str())
            .map(PlanApproval::from_status)
    });
    match approval {
        Some(approval) => SubmitResult { approval, plan_id },
        None => {
            // A non-JSON or status-less body is conservatively a Timeout, but that
            // makes a SERVER ERROR indistinguishable from a genuine human timeout.
            // Emit a bounded diagnostic (a short prefix only — never the whole
            // untrusted body, to keep logs clean) so the operator can tell them
            // apart. No behavior change: still Timeout, and (conservatively) no
            // create on the board.
            eprintln!(
                "devboule planner: plan_submit returned no usable `status` \
                 (treating as timeout); body prefix: {:?}",
                truncate_chars(text, 200)
            );
            SubmitResult {
                approval: PlanApproval::Timeout,
                plan_id,
            }
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
    // Returns a fixed spine for `project_structure`, the REAL `oracle_context` JSON
    // shape (`{query, indexStatus, chunks:[...]}`) for EVERY oracle_context call
    // (configurable chunks; empty by default so the new GOAL-grounding call adds
    // NOTHING unless a test opts in — existing EXPLORE/PLAN call counts stay
    // untouched), and a configurable status for `plan_submit`; records every call
    // so tests can assert the tool sequence + payloads.

    struct MockMcp {
        spine_paths: Vec<String>,
        submit_status: String,
        oracle_chunks: Vec<(String, String)>,
        oracle_context_fails: bool,
        calls: Mutex<Vec<(String, serde_json::Value)>>,
    }
    impl MockMcp {
        fn new(spine_paths: Vec<&str>, submit_status: &str) -> Self {
            Self {
                spine_paths: spine_paths.into_iter().map(|s| s.to_string()).collect(),
                submit_status: submit_status.to_string(),
                oracle_chunks: Vec::new(),
                oracle_context_fails: false,
                calls: Mutex::new(Vec::new()),
            }
        }
        /// Every `oracle_context` call (goal-grounding AND per-file EXPLORE
        /// grounding) returns these `(file_source, text)` pairs as chunks.
        fn with_oracle_chunks(mut self, chunks: Vec<(&str, &str)>) -> Self {
            self.oracle_chunks = chunks
                .into_iter()
                .map(|(f, t)| (f.to_string(), t.to_string()))
                .collect();
            self
        }
        /// Every `oracle_context` call fails (tests the best-effort skip path).
        fn failing_oracle_context(mut self) -> Self {
            self.oracle_context_fails = true;
            self
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
            self.calls
                .lock()
                .unwrap()
                .push((name.to_string(), params.clone()));
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
                "oracle_context" => {
                    if self.oracle_context_fails {
                        return Err("oracle_context unavailable".to_string());
                    }
                    let chunks: Vec<serde_json::Value> = self
                        .oracle_chunks
                        .iter()
                        .map(|(file_source, text)| {
                            serde_json::json!({
                                "chunk_id": "c1",
                                "file_source": file_source,
                                "text": text,
                                "score": 0.9,
                            })
                        })
                        .collect();
                    Ok(serde_json::json!({
                        "query": params.get("query").and_then(|v| v.as_str()).unwrap_or(""),
                        "indexStatus": {"ready": true},
                        "chunks": chunks,
                    })
                    .to_string())
                }
                "plan_submit" => Ok(
                    serde_json::json!({"planId": "p1", "status": self.submit_status}).to_string(),
                ),
                "project_create_plan_tasks" => {
                    // Echo back the planId we were sent + a minimal tasks array, matching
                    // the 1a contract's `{project, planId, idMap, tasks}` shape (the
                    // planner only reads `planId`).
                    let plan_id = params
                        .get("plan_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("p1")
                        .to_string();
                    Ok(serde_json::json!({
                        "project": {"id": "proj"},
                        "planId": plan_id,
                        "idMap": {},
                        "tasks": [],
                    })
                    .to_string())
                }
                other => Err(format!("unexpected tool {other}")),
            }
        }
    }

    /// A disabled (no-op) activity emitter for the tests that don't assert the
    /// milestone stream — they exercise the planner exactly as before the milestones
    /// were threaded in (no file is touched).
    fn noop_activity() -> Activity {
        Activity::disabled()
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

    // --- Happy path: STRUCTURE -> EXPLORE -> PLAN -> submit -> create on board ---

    #[tokio::test]
    async fn full_planner_run_creates_on_the_board_and_submits() {
        let (_dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n"), ("src/b.rs", "fn b() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs", "src/b.rs"], "approved");
        // One EXPLORE note per spine file (2), then one PLAN block.
        let model = CapturingModel::new(vec![
            note_block("src/a.rs"),
            note_block("src/b.rs"),
            plan_block(one_valid_task()),
        ]);

        let outcome = run_planner("do the thing", &model, &mcp, &fs, "proj", &noop_activity(), true)
            .await
            .expect("planner succeeds");

        // (1) calls project_structure FIRST.
        let names = mcp.call_names();
        assert_eq!(names.first().map(|s| s.as_str()), Some("project_structure"));
        // (6) on approval, the LAST call is the Kanban bulk-create (after plan_submit).
        assert_eq!(
            names.last().map(|s| s.as_str()),
            Some("project_create_plan_tasks")
        );
        // plan_submit precedes the create (the human gate happens BEFORE the board write).
        let submit_pos = names.iter().position(|n| n == "plan_submit").unwrap();
        let create_pos = names
            .iter()
            .position(|n| n == "project_create_plan_tasks")
            .unwrap();
        assert!(
            submit_pos < create_pos,
            "plan_submit precedes the board create: {names:?}"
        );

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
        assert!(
            !explore_a.contains("fn b()"),
            "explore A must not carry B's body"
        );
        assert!(explore_b.contains("src/b.rs") && explore_b.contains("fn b()"));
        assert!(
            !explore_b.contains("fn a()"),
            "explore B must not carry A's body"
        );
        assert!(
            explore_a.chars().count() <= MAX_EXPLORE_PROMPT_CHARS,
            "explore prompt is bounded"
        );

        // The PLAN prompt carries the goal + the accumulated notes, NOT raw file
        // bodies.
        let plan_prompt = &prompts[2];
        assert!(
            plan_prompt.contains("do the thing"),
            "plan prompt carries the goal"
        );
        assert!(
            plan_prompt.contains("central module"),
            "plan prompt carries the notes"
        );
        assert!(
            !plan_prompt.contains("fn a()"),
            "plan prompt carries NO raw file body"
        );

        // (4) a VALID TasksPlan with the expected task.
        assert_eq!(outcome.tasks_plan.tasks.len(), 1);
        assert_eq!(outcome.tasks_plan.tasks[0].id, "T001");
        assert_eq!(outcome.approval, PlanApproval::Approved);
        // The outcome carries the planId the tasks were created under (from plan_submit).
        assert_eq!(outcome.plan_id.as_deref(), Some("p1"));

        // (5) the bulk-create payload tags the tasks with the planId from plan_submit
        // and sends the planner's INTERNAL ids + scope/acceptance/dependsOn (camelCase).
        let create = mcp.last_of("project_create_plan_tasks").unwrap();
        assert_eq!(create["project_id"], serde_json::json!("proj"));
        assert_eq!(
            create["plan_id"],
            serde_json::json!("p1"),
            "tagged with the planId"
        );
        let created_tasks = create["tasks"].as_array().unwrap();
        assert_eq!(created_tasks.len(), 1);
        assert_eq!(created_tasks[0]["id"], serde_json::json!("T001"));
        assert_eq!(created_tasks[0]["scope"], serde_json::json!(["src/a.rs"]));
        assert_eq!(
            created_tasks[0]["acceptance"],
            serde_json::json!("cargo test passes")
        );
        assert_eq!(created_tasks[0]["dependsOn"], serde_json::json!([]));
        // The runtime-only fields are NOT sent: the board owns status/attempts.
        assert!(
            created_tasks[0].get("status").is_none(),
            "no status in the create payload"
        );
        assert!(
            created_tasks[0].get("attempts").is_none(),
            "no attempts in the create payload"
        );

        // The plan_submit payload carries project_id + title + plan_markdown.
        let submit = mcp.last_of("plan_submit").unwrap();
        assert_eq!(submit["project_id"], serde_json::json!("proj"));
        assert!(submit["title"].as_str().unwrap().contains("Devboule plan"));
        assert!(submit["plan_markdown"].as_str().unwrap().contains("T001"));
    }

    #[tokio::test]
    async fn rejected_status_maps_to_rejected_and_creates_nothing() {
        let (_dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs"], "rejected");
        let model = CapturingModel::new(vec![note_block("src/a.rs"), plan_block(one_valid_task())]);

        let outcome = run_planner("g", &model, &mcp, &fs, "proj", &noop_activity(), true)
            .await
            .expect("planner succeeds even when the human rejects");
        assert_eq!(outcome.approval, PlanApproval::Rejected);
        // A rejected plan never touches the board: no planId, no bulk-create call.
        assert_eq!(outcome.plan_id, None);
        assert!(
            !mcp.call_names()
                .iter()
                .any(|n| n == "project_create_plan_tasks"),
            "a rejected plan must NOT be created on the board"
        );
    }

    // --- Milestone stream (the LIVE Console payoff) --------------------------

    /// Read the activity file back as parsed (text, node) milestone tuples.
    fn read_milestones(file: &std::path::Path) -> Vec<(String, String)> {
        let body = std::fs::read_to_string(file).unwrap_or_default();
        body.lines()
            .map(|line| {
                let v: serde_json::Value = serde_json::from_str(line).expect("each line is JSON");
                assert_eq!(v["kind"], "milestone", "every event is a milestone");
                (
                    v["text"].as_str().unwrap().to_string(),
                    v["node"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn planner_emits_the_expected_milestone_sequence() {
        // Drive a full happy-path plan with a real (file-backed) Activity emitter and
        // assert the EXACT coder-tier milestone sequence the Console will show live:
        // STRUCTURE -> one EXPLORE per spine file -> PLAN drafted -> SUBMIT -> approved ->
        // tasks created on the board.
        let (dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n"), ("src/b.rs", "fn b() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs", "src/b.rs"], "approved");
        let model = CapturingModel::new(vec![
            note_block("src/a.rs"),
            note_block("src/b.rs"),
            plan_block(one_valid_task()),
        ]);
        let act_file = dir.path().join("activity.jsonl");
        let activity = Activity::with_path(&act_file);

        run_planner("do the thing", &model, &mcp, &fs, "proj", &activity, true)
            .await
            .expect("planner succeeds");

        let milestones = read_milestones(&act_file);
        assert_eq!(
            milestones,
            vec![
                ("Planning: 2 spine files".to_string(), "dot".to_string()),
                ("grounding goal via oracle".to_string(), "".to_string()),
                ("exploring a.rs".to_string(), "".to_string()),
                ("exploring b.rs".to_string(), "".to_string()),
                ("drafted 1 tasks".to_string(), "dot".to_string()),
                (
                    "plan submitted — awaiting approval".to_string(),
                    "terra".to_string()
                ),
                ("plan approved".to_string(), "sage".to_string()),
                (
                    "1 task(s) created on the board".to_string(),
                    "sage".to_string()
                ),
            ],
            "full ordered milestone stream"
        );
    }

    #[tokio::test]
    async fn auto_create_off_approves_the_plan_but_creates_no_tasks() {
        // The composer's "auto-create: off" (auto_create=false): an APPROVED plan is NOT turned into
        // board tasks — the terminal milestone says so explicitly and the outcome carries no created
        // plan id (no `project_create_plan_tasks` call).
        let (dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs"], "approved");
        let model = CapturingModel::new(vec![note_block("src/a.rs"), plan_block(one_valid_task())]);
        let act_file = dir.path().join("activity.jsonl");
        let activity = Activity::with_path(&act_file);

        let outcome = run_planner("g", &model, &mcp, &fs, "proj", &activity, false)
            .await
            .expect("planner succeeds");

        assert!(
            outcome.plan_id.is_none(),
            "auto-create off ⇒ no tasks created ⇒ no created plan id"
        );
        let milestones = read_milestones(&act_file);
        let last = milestones.last().expect("at least one milestone");
        assert!(
            last.0.contains("NOT auto-created"),
            "auto-create off ⇒ terminal milestone notes tasks were not created, got {last:?}"
        );
        assert!(
            !milestones.iter().any(|(t, _)| t.contains("created on the board")),
            "auto-create off ⇒ no 'created on the board' milestone"
        );
    }

    #[tokio::test]
    async fn rejected_plan_emits_a_rejected_milestone() {
        let (dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs"], "rejected");
        let model = CapturingModel::new(vec![note_block("src/a.rs"), plan_block(one_valid_task())]);
        let act_file = dir.path().join("activity.jsonl");
        let activity = Activity::with_path(&act_file);

        run_planner("g", &model, &mcp, &fs, "proj", &activity, true)
            .await
            .unwrap();

        let milestones = read_milestones(&act_file);
        // The terminal milestone reflects the human verdict (rejected -> terra ring).
        assert_eq!(
            milestones.last(),
            Some(&("plan rejected".to_string(), "terra".to_string()))
        );
        // The submit milestone is still emitted before the verdict.
        assert!(milestones
            .iter()
            .any(|(t, n)| t == "plan submitted — awaiting approval" && n == "terra"));
    }

    #[tokio::test]
    async fn skipped_explore_emits_no_milestone_for_that_file() {
        // A spine file that does not exist on disk is skipped (no model call, no
        // EXPLORE milestone) — proof the per-EXPLORE milestone tracks ACTUAL work.
        let (dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs", "src/ghost.rs"], "approved");
        let model = CapturingModel::new(vec![note_block("src/a.rs"), plan_block(one_valid_task())]);
        let act_file = dir.path().join("activity.jsonl");
        let activity = Activity::with_path(&act_file);

        run_planner("g", &model, &mcp, &fs, "proj", &activity, true)
            .await
            .unwrap();

        let milestones = read_milestones(&act_file);
        let explore_count = milestones
            .iter()
            .filter(|(t, _)| t.starts_with("exploring "))
            .count();
        assert_eq!(
            explore_count, 1,
            "only the real file emits an explore milestone"
        );
        // The STRUCTURE milestone still counts BOTH spine files (it is the spine size).
        assert_eq!(milestones[0].0, "Planning: 2 spine files");
    }

    #[test]
    fn path_basename_extracts_the_trailing_component() {
        assert_eq!(path_basename("src/backend/projects.rs"), "projects.rs");
        assert_eq!(path_basename("main.rs"), "main.rs");
        // A trailing slash falls back to the last non-empty segment.
        assert_eq!(path_basename("src/dir/"), "dir");
        // No usable segment -> the whole string (never empty).
        assert_eq!(path_basename(""), "");
        assert_eq!(path_basename("/"), "/");
    }

    #[tokio::test]
    async fn unknown_submit_status_is_conservative_timeout() {
        // Any non-approved/non-rejected status (e.g. a server "vanished") must map
        // to Timeout — never a false Approved.
        let (_dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs"], "vanished");
        let model = CapturingModel::new(vec![note_block("src/a.rs"), plan_block(one_valid_task())]);
        let outcome = run_planner("g", &model, &mcp, &fs, "proj", &noop_activity(), true)
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
        let (_dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs", "src/ghost.rs"], "approved");
        let model = CapturingModel::new(vec![note_block("src/a.rs"), plan_block(one_valid_task())]);
        let outcome = run_planner("g", &model, &mcp, &fs, "proj", &noop_activity(), true)
            .await
            .unwrap();
        // Only ONE explore call (ghost skipped) + ONE plan call.
        assert_eq!(
            model.prompts().len(),
            2,
            "ghost file => no explore call for it"
        );
        assert_eq!(outcome.tasks_plan.tasks.len(), 1);
    }

    #[tokio::test]
    async fn malformed_explore_note_is_skipped_plan_still_made() {
        // A model that emits garbage for one note must not sink the whole plan.
        let (_dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs"], "approved");
        let model = CapturingModel::new(vec![
            "not a note block at all".to_string(),
            plan_block(one_valid_task()),
        ]);
        let outcome = run_planner("g", &model, &mcp, &fs, "proj", &noop_activity(), true)
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
        // Each rendered note line is "- <source>: <role>" + truncation; pick a role
        // length that pushes every note to EXACTLY MAX_NOTE_CHARS after truncation
        // so the budget math is exact, then pick N so the overflow lands on the
        // LAST spine file: after k notes the running total is
        // `k*MAX_NOTE_CHARS + (k-1)` join newlines, so N is the smallest count
        // whose total exceeds MAX_NOTES_TOTAL_CHARS. This keeps the test scaling
        // automatically with either budget constant instead of hardcoding N
        // against a since-reduced MAX_NOTES_TOTAL_CHARS.
        let role_len = MAX_NOTE_CHARS; // render_note truncates the line to MAX_NOTE_CHARS
        let n = MAX_NOTES_TOTAL_CHARS / (MAX_NOTE_CHARS + 1) + 1;
        assert!(
            n <= MAX_SPINE,
            "test assumption: N must fit within one structural spine (got {n})"
        );

        let paths: Vec<String> = (0..n).map(|i| format!("src/f{i}.rs")).collect();
        let files: Vec<(&str, &str)> = paths.iter().map(|p| (p.as_str(), "fn x() {}\n")).collect();
        let (_dir, fs) = fs_with_files(&files);
        let mcp = MockMcp::new(paths.iter().map(|s| s.as_str()).collect(), "approved");

        let mut outputs: Vec<String> = paths
            .iter()
            .map(|p| note_block_with_role(p, role_len))
            .collect();
        outputs.push(plan_block(one_valid_task()));
        let model = CapturingModel::new(outputs);

        run_planner("g", &model, &mcp, &fs, "proj", &noop_activity(), true)
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

    // --- oracle_context grounding: structured PROSE, not raw JSON -----------

    #[test]
    fn build_grounding_text_parses_real_oracle_json_into_prose() {
        let raw = serde_json::json!({
            "query": "role of src/a.rs",
            "indexStatus": {"ready": true},
            "chunks": [
                {"chunk_id": "c1", "file_source": "src/a.rs", "text": "does the a-thing", "score": 0.9},
                {"chunk_id": "c2", "file_source": "src/b.rs", "text": "does the b-thing", "score": 0.5},
            ],
        })
        .to_string();
        let grounding = build_grounding_text(&raw, MAX_GROUNDING_CHARS);
        assert!(
            grounding.contains("-- src/a.rs\ndoes the a-thing"),
            "chunk 1 rendered as prose: {grounding}"
        );
        assert!(
            grounding.contains("-- src/b.rs\ndoes the b-thing"),
            "chunk 2 rendered as prose: {grounding}"
        );
        assert!(
            !grounding.contains("chunk_id") && !grounding.contains("indexStatus"),
            "no raw JSON scaffolding leaks into the prose: {grounding}"
        );
    }

    #[test]
    fn build_grounding_text_falls_back_to_raw_truncate_on_non_json() {
        // The Oracle server may change shape; a non-JSON (or wrongly-shaped) result
        // must degrade to the PRE-fix behavior (raw char-truncate), never panic and
        // never silently produce empty grounding when there was real content.
        let raw = "half-truncated garbage that is definitely not JSON {\"chunks\":";
        let grounding = build_grounding_text(raw, MAX_GROUNDING_CHARS);
        assert_eq!(
            grounding, raw,
            "non-JSON input is passed through (char-truncated) unchanged"
        );
    }

    #[test]
    fn build_grounding_text_respects_the_char_cap() {
        let raw = serde_json::json!({
            "chunks": [{"file_source": "src/a.rs", "text": "x".repeat(10_000)}],
        })
        .to_string();
        let grounding = build_grounding_text(&raw, 50);
        assert!(grounding.chars().count() <= 50, "grounding must respect the cap");
    }

    // --- GOAL-driven spine grounding (Piece 2) -------------------------------

    #[test]
    fn goal_spine_entries_extracts_new_safe_paths_capped_and_deduped() {
        let existing: HashSet<String> = ["src/a.rs".to_string()].into_iter().collect();
        let result = OracleContextResult {
            chunks: vec![
                OracleChunk { file_source: "src/a.rs".to_string(), text: "already in spine".to_string() },
                OracleChunk { file_source: "src/new1.rs".to_string(), text: "t".to_string() },
                OracleChunk { file_source: "src/new1.rs".to_string(), text: "duplicate chunk".to_string() },
                OracleChunk { file_source: "../escape.rs".to_string(), text: "t".to_string() },
                OracleChunk { file_source: "/etc/passwd".to_string(), text: "t".to_string() },
                OracleChunk { file_source: "src/new2.rs".to_string(), text: "t".to_string() },
                OracleChunk { file_source: "src/new3.rs".to_string(), text: "t".to_string() },
                OracleChunk { file_source: "src/new4.rs".to_string(), text: "t".to_string() },
                OracleChunk { file_source: "src/new5.rs".to_string(), text: "t".to_string() },
            ],
        };
        let entries = goal_spine_entries(&result, &existing);
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["src/new1.rs", "src/new2.rs", "src/new3.rs", "src/new4.rs"],
            "dedup (against spine + within itself), unsafe paths filtered, capped at MAX_GOAL_SPINE"
        );
        assert_eq!(entries.len(), MAX_GOAL_SPINE);
        for e in &entries {
            assert_eq!(e.in_degree, 0);
            assert!(e.top_symbols.is_empty());
        }
    }

    #[test]
    fn goal_spine_entries_excludes_dot_leading_harness_paths() {
        // The planner must never EXPLORE its own harness/tool scratchpads — least of
        // all its OWN preplan file, which would let a planning session's scratch
        // notes get grounded back into ITS OWN future EXPLORE prompts as if they
        // were project source. `check_rel_path` alone does not reject a dot-leading
        // component (only "..", absolute paths, and a leading '-'), so this is an
        // independent filter.
        let existing: HashSet<String> = HashSet::new();
        let result = OracleContextResult {
            chunks: vec![
                OracleChunk { file_source: ".devboule/preplan.md".to_string(), text: "t".to_string() },
                OracleChunk { file_source: ".git/config".to_string(), text: "t".to_string() },
                OracleChunk { file_source: "src/real.rs".to_string(), text: "t".to_string() },
            ],
        };
        let entries = goal_spine_entries(&result, &existing);
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["src/real.rs"],
            "dot-leading component paths must be filtered out: {paths:?}"
        );
    }

    #[tokio::test]
    async fn goal_chunks_add_new_files_to_exploration() {
        let (_dir, fs) = fs_with_files(&[
            ("src/a.rs", "fn a() {}\n"),
            ("src/goalfile.rs", "fn goal() {}\n"),
        ]);
        let mcp = MockMcp::new(vec!["src/a.rs"], "approved")
            .with_oracle_chunks(vec![("src/goalfile.rs", "goal-relevant context")]);
        let model = CapturingModel::new(vec![
            note_block("src/a.rs"),
            note_block("src/goalfile.rs"),
            plan_block(one_valid_task()),
        ]);
        let outcome = run_planner("do the thing", &model, &mcp, &fs, "proj", &noop_activity(), true)
            .await
            .expect("planner succeeds");
        let prompts = model.prompts();
        assert_eq!(prompts.len(), 3, "1 structural + 1 goal-added EXPLORE + 1 PLAN call");
        assert!(
            prompts[1].contains("src/goalfile.rs") && prompts[1].contains("fn goal()"),
            "the goal-added file gets its OWN EXPLORE prompt: {}",
            prompts[1]
        );
        assert_eq!(outcome.tasks_plan.tasks.len(), 1);
    }

    #[tokio::test]
    async fn goal_chunks_dedup_against_the_existing_spine() {
        let (_dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs"], "approved")
            .with_oracle_chunks(vec![("src/a.rs", "already in spine, adds nothing")]);
        let model = CapturingModel::new(vec![note_block("src/a.rs"), plan_block(one_valid_task())]);
        run_planner("g", &model, &mcp, &fs, "proj", &noop_activity(), true)
            .await
            .unwrap();
        assert_eq!(
            model.prompts().len(),
            2,
            "a goal chunk duplicating the structural spine adds no extra EXPLORE call"
        );
    }

    #[tokio::test]
    async fn goal_chunks_are_capped_at_max_goal_spine() {
        let goal_paths: Vec<String> = (0..6).map(|i| format!("src/goal{i}.rs")).collect();
        let goal_bodies: Vec<String> = (0..6).map(|i| format!("fn g{i}() {{}}\n")).collect();
        let mut files: Vec<(&str, &str)> = vec![("src/a.rs", "fn a() {}\n")];
        for i in 0..6 {
            files.push((goal_paths[i].as_str(), goal_bodies[i].as_str()));
        }
        let (_dir, fs) = fs_with_files(&files);
        let mcp = MockMcp::new(vec!["src/a.rs"], "approved")
            .with_oracle_chunks(goal_paths.iter().map(|p| (p.as_str(), "ctx")).collect());

        let mut outputs = vec![note_block("src/a.rs")];
        for path in goal_paths.iter().take(MAX_GOAL_SPINE) {
            outputs.push(note_block(path));
        }
        outputs.push(plan_block(one_valid_task()));
        let model = CapturingModel::new(outputs);

        run_planner("g", &model, &mcp, &fs, "proj", &noop_activity(), true)
            .await
            .expect("planner succeeds");
        assert_eq!(
            model.prompts().len(),
            1 + MAX_GOAL_SPINE + 1,
            "1 structural + MAX_GOAL_SPINE capped goal files + 1 PLAN call"
        );
    }

    #[tokio::test]
    async fn goal_grounding_oracle_failure_is_skipped_not_fatal() {
        let (_dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs"], "approved").failing_oracle_context();
        let model = CapturingModel::new(vec![note_block("src/a.rs"), plan_block(one_valid_task())]);
        let outcome = run_planner("g", &model, &mcp, &fs, "proj", &noop_activity(), true)
            .await
            .expect("an oracle_context failure must be non-fatal");
        assert_eq!(
            model.prompts().len(),
            2,
            "identical to today: 1 EXPLORE + 1 PLAN when oracle_context is unavailable"
        );
        assert_eq!(outcome.tasks_plan.tasks.len(), 1);
    }

    #[tokio::test]
    async fn goal_chunks_with_unsafe_paths_are_filtered() {
        let (_dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs"], "approved")
            .with_oracle_chunks(vec![("../escape.rs", "x"), ("/etc/passwd", "y")]);
        let model = CapturingModel::new(vec![note_block("src/a.rs"), plan_block(one_valid_task())]);
        run_planner("g", &model, &mcp, &fs, "proj", &noop_activity(), true)
            .await
            .unwrap();
        assert_eq!(
            model.prompts().len(),
            2,
            "unsafe goal chunk paths must be filtered out, adding nothing to explore"
        );
    }

    // --- PRE-PLAN NOTES external memory wiring (Piece 3) ---------------------

    #[tokio::test]
    async fn preplan_findings_feed_into_the_plan_prompt() {
        let (_dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs"], "approved");
        let model = CapturingModel::new(vec![note_block("src/a.rs"), plan_block(one_valid_task())]);
        run_planner("g", &model, &mcp, &fs, "proj", &noop_activity(), true)
            .await
            .unwrap();
        let prompts = model.prompts();
        let plan_prompt = prompts.last().unwrap();
        assert!(plan_prompt.contains("PRE-PLAN NOTES"), "{plan_prompt}");
        assert!(
            plan_prompt.contains("STRUCTURE spine: src/a.rs"),
            "the STRUCTURE finding is fed back in: {plan_prompt}"
        );
        assert!(
            plan_prompt.contains("central module"),
            "the EXPLORE note's role text is fed back in via PRE-PLAN NOTES too: {plan_prompt}"
        );
    }

    #[tokio::test]
    async fn preexisting_preplan_with_the_same_goal_resumes_into_the_plan_prompt() {
        // Simulates a CRASHED prior run: a preplan.md already exists for the SAME
        // goal, with a finding the current run never (re-)discovered on its own.
        let (dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let preplan_path = dir.path().join(".devboule").join("preplan.md");
        std::fs::create_dir_all(preplan_path.parent().unwrap()).unwrap();
        std::fs::write(
            &preplan_path,
            "## Goal\ng\n\n## Constraints\n\n## Findings\n- PRIOR RUN: something important already learned\n\n## Decisions\n\n## Open questions\n\n## Draft outline\n",
        )
        .unwrap();

        let mcp = MockMcp::new(vec!["src/a.rs"], "approved");
        let model = CapturingModel::new(vec![note_block("src/a.rs"), plan_block(one_valid_task())]);
        run_planner("g", &model, &mcp, &fs, "proj", &noop_activity(), true)
            .await
            .unwrap();

        let prompts = model.prompts();
        let plan_prompt = prompts.last().unwrap();
        assert!(
            plan_prompt.contains("PRIOR RUN: something important already learned"),
            "the crash-resumed finding must appear in the PLAN prompt: {plan_prompt}"
        );
    }

    #[tokio::test]
    async fn plan_retry_rejection_is_recorded_in_preplan_decisions() {
        // Uses an UNKNOWN submit status ("vanished" -> Timeout, NOT terminal) so the
        // preplan file survives the run and can be inspected afterward.
        let (dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs"], "vanished");
        let four_scope = serde_json::json!([{
            "id": "T001", "title": "too big",
            "scope": ["a.rs", "b.rs", "c.rs", "d.rs"],
            "acceptance": "x", "dependsOn": [], "status": "pending", "attempts": 0,
        }]);
        let model = CapturingModel::new(vec![
            note_block("src/a.rs"),
            plan_block(four_scope),       // attempt 1: rejected (scope > 3)
            plan_block(one_valid_task()), // attempt 2: valid
        ]);
        let outcome = run_planner("g", &model, &mcp, &fs, "proj", &noop_activity(), true)
            .await
            .unwrap();
        assert_eq!(outcome.approval, PlanApproval::Timeout);

        let preplan_path = dir.path().join(".devboule").join("preplan.md");
        let body = std::fs::read_to_string(&preplan_path)
            .expect("a non-terminal (Timeout) verdict must NOT clear the preplan file");
        assert!(
            body.contains("attempt 1 rejected") && body.contains("scope"),
            "records the rejection reason under Decisions: {body}"
        );
    }

    #[tokio::test]
    async fn approved_plan_clears_the_preplan_file() {
        let (dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs"], "approved");
        let model = CapturingModel::new(vec![note_block("src/a.rs"), plan_block(one_valid_task())]);
        run_planner("g", &model, &mcp, &fs, "proj", &noop_activity(), true)
            .await
            .unwrap();
        let preplan_path = dir.path().join(".devboule").join("preplan.md");
        assert!(
            !preplan_path.exists(),
            "an approved (terminal) plan must clear its preplan memory"
        );
    }

    #[tokio::test]
    async fn rejected_plan_clears_the_preplan_file_too() {
        let (dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs"], "rejected");
        let model = CapturingModel::new(vec![note_block("src/a.rs"), plan_block(one_valid_task())]);
        run_planner("g", &model, &mcp, &fs, "proj", &noop_activity(), true)
            .await
            .unwrap();
        let preplan_path = dir.path().join(".devboule").join("preplan.md");
        assert!(
            !preplan_path.exists(),
            "a rejected (terminal) plan must also clear its preplan memory"
        );
    }

    // --- PLAN validation (reject + retry feeds the error back) ---------------

    #[tokio::test]
    async fn four_file_scope_is_rejected_then_retry_succeeds() {
        let (_dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs"], "approved");
        let four_scope = serde_json::json!([{
            "id": "T001", "title": "too big",
            "scope": ["a.rs", "b.rs", "c.rs", "d.rs"],
            "acceptance": "x", "dependsOn": [], "status": "pending", "attempts": 0,
        }]);
        let model = CapturingModel::new(vec![
            note_block("src/a.rs"),
            plan_block(four_scope),       // attempt 1: rejected (scope > 3)
            plan_block(one_valid_task()), // attempt 2: valid
        ]);
        let outcome = run_planner("g", &model, &mcp, &fs, "proj", &noop_activity(), true)
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

    // --- ROLE UNTANGLE Phase 4: task weight ----------------------------------

    fn weighted_task(id: &str, weight: &str) -> Task {
        Task {
            id: id.into(),
            title: "t".into(),
            scope: vec!["a.rs".into()],
            context_files: vec![],
            acceptance: "builds".into(),
            depends_on: vec![],
            status: "pending".into(),
            attempts: 0,
            weight: weight.into(),
        }
    }

    #[test]
    fn weight_validation_accepts_known_tiers_and_rejects_typos() {
        for w in ["", "mini", "main"] {
            let plan = TasksPlan {
                project_goal: "g".into(),
                tasks: vec![weighted_task("T001", w)],
            };
            assert!(
                validate_plan_structure(&plan).is_ok(),
                "weight {w:?} must be accepted"
            );
        }
        let plan = TasksPlan {
            project_goal: "g".into(),
            tasks: vec![weighted_task("T001", "heavy")],
        };
        let err = validate_plan_structure(&plan).expect_err("unknown weight rejected");
        assert!(
            err.contains("invalid weight") && err.contains("T001"),
            "names the field + task: {err}"
        );
    }

    #[test]
    fn payload_emits_weight_only_for_main_no_churn() {
        let plan = TasksPlan {
            project_goal: "g".into(),
            tasks: vec![
                weighted_task("T001", "main"),
                weighted_task("T002", "mini"),
                weighted_task("T003", ""),
            ],
        };
        let payload = build_plan_tasks_payload(&plan);
        let arr = payload.as_array().unwrap();
        assert_eq!(arr[0]["weight"], serde_json::json!("main"));
        // NO-CHURN: a mini / unweighted task carries no `weight` key at all.
        assert!(arr[1].get("weight").is_none(), "mini omits weight: {:?}", arr[1]);
        assert!(arr[2].get("weight").is_none(), "empty omits weight: {:?}", arr[2]);
    }

    #[test]
    fn weight_deserializes_null_and_absent_as_empty() {
        // A local model may omit the key OR emit `"weight": null` — both must
        // normalize to "" (mini), never a parse error that burns a plan retry.
        let absent: Task = serde_json::from_str(
            r#"{"id":"T1","title":"t","scope":["a.rs"],"acceptance":"b","status":"pending","attempts":0}"#,
        )
        .expect("absent weight parses");
        assert_eq!(absent.weight, "");
        let null: Task = serde_json::from_str(
            r#"{"id":"T1","title":"t","scope":["a.rs"],"acceptance":"b","status":"pending","attempts":0,"weight":null}"#,
        )
        .expect("null weight parses");
        assert_eq!(null.weight, "");
        let main: Task = serde_json::from_str(
            r#"{"id":"T1","title":"t","scope":["a.rs"],"acceptance":"b","status":"pending","attempts":0,"weight":"main"}"#,
        )
        .expect("main weight parses");
        assert_eq!(main.weight, "main");
    }

    #[tokio::test]
    async fn dependson_cycle_is_rejected() {
        let plan = TasksPlan {
            project_goal: "g".into(),
            tasks: vec![
                Task {
                    id: "T001".into(),
                    title: "a".into(),
                    scope: vec!["a.rs".into()],
                    context_files: vec![],
                    acceptance: "x".into(),
                    depends_on: vec!["T002".into()],
                    status: "pending".into(),
                    attempts: 0,
                    weight: String::new(),
                },
                Task {
                    id: "T002".into(),
                    title: "b".into(),
                    scope: vec!["b.rs".into()],
                    context_files: vec![],
                    acceptance: "x".into(),
                    depends_on: vec!["T001".into()],
                    status: "pending".into(),
                    attempts: 0,
                    weight: String::new(),
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
                id: "T001".into(),
                title: "a".into(),
                scope: vec!["a.rs".into()],
                context_files: vec![],
                acceptance: "x".into(),
                depends_on: vec!["T999".into()],
                status: "pending".into(),
                attempts: 0,
                weight: String::new(),
            }],
        };
        let err = validate_plan(&plan).expect_err("a dangling dep must be rejected");
        assert!(
            err.contains("unknown task id"),
            "names the dangling dep: {err}"
        );
    }

    #[tokio::test]
    async fn empty_acceptance_is_rejected() {
        let plan = TasksPlan {
            project_goal: "g".into(),
            tasks: vec![Task {
                id: "T001".into(),
                title: "a".into(),
                scope: vec!["a.rs".into()],
                context_files: vec![],
                acceptance: "   ".into(),
                depends_on: vec![],
                status: "pending".into(),
                attempts: 0,
                weight: String::new(),
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
                    id: "T001".into(),
                    title: "a".into(),
                    scope: vec!["a.rs".into()],
                    context_files: vec![],
                    acceptance: "x".into(),
                    depends_on: vec![],
                    status: "pending".into(),
                    attempts: 0,
                    weight: String::new(),
                },
                Task {
                    id: "T001".into(),
                    title: "b".into(),
                    scope: vec!["b.rs".into()],
                    context_files: vec![],
                    acceptance: "x".into(),
                    depends_on: vec![],
                    status: "pending".into(),
                    attempts: 0,
                    weight: String::new(),
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
                    id: "T001".into(),
                    title: "a".into(),
                    scope: vec!["a.rs".into()],
                    context_files: vec![],
                    acceptance: "x".into(),
                    depends_on: vec![],
                    status: "pending".into(),
                    attempts: 0,
                    weight: String::new(),
                },
                Task {
                    id: "T002".into(),
                    title: "b".into(),
                    scope: vec!["b.rs".into()],
                    context_files: vec![],
                    acceptance: "x".into(),
                    depends_on: vec!["T001".into(), "T001".into()],
                    status: "pending".into(),
                    attempts: 0,
                    weight: String::new(),
                },
            ],
        };
        let err = validate_plan(&plan).expect_err("a duplicate dep must be rejected");
        assert!(
            err.contains("duplicate dependsOn"),
            "names the duplicate dep: {err}"
        );
    }

    #[tokio::test]
    async fn nonzero_attempts_rejected() {
        // A freshly drafted task MUST start with attempts == 0; a non-zero counter is a
        // corrupted draft (the board has no attempts field; the runner counts in-run).
        let plan = TasksPlan {
            project_goal: "g".into(),
            tasks: vec![Task {
                id: "T001".into(),
                title: "a".into(),
                scope: vec!["a.rs".into()],
                context_files: vec![],
                acceptance: "x".into(),
                depends_on: vec![],
                status: "pending".into(),
                attempts: 1,
                weight: String::new(),
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
                id: "T001".into(),
                title: "a".into(),
                scope: vec!["../escape.rs".into()],
                context_files: vec![],
                acceptance: "x".into(),
                depends_on: vec![],
                status: "pending".into(),
                attempts: 0,
                weight: String::new(),
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
                id: "T001".into(),
                title: "a".into(),
                scope: vec!["a.rs".into()],
                context_files: vec![],
                acceptance: "x".into(),
                depends_on: vec!["T001".into()],
                status: "pending".into(),
                attempts: 0,
                weight: String::new(),
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
                Task {
                    id: "T001".into(),
                    title: "a".into(),
                    scope: vec!["a.rs".into()],
                    context_files: vec![],
                    acceptance: "x".into(),
                    depends_on: vec![],
                    status: "pending".into(),
                    attempts: 0,
                    weight: String::new(),
                },
                Task {
                    id: "T002".into(),
                    title: "b".into(),
                    scope: vec!["b.rs".into()],
                    context_files: vec![],
                    acceptance: "x".into(),
                    depends_on: vec!["T001".into()],
                    status: "pending".into(),
                    attempts: 0,
                    weight: String::new(),
                },
                Task {
                    id: "T003".into(),
                    title: "c".into(),
                    scope: vec!["c.rs".into()],
                    context_files: vec![],
                    acceptance: "x".into(),
                    depends_on: vec!["T002".into()],
                    status: "pending".into(),
                    attempts: 0,
                    weight: String::new(),
                },
            ],
        };
        assert!(validate_plan(&plan).is_ok(), "a linear DAG is valid");
    }

    #[tokio::test]
    async fn flat_exhaustion_always_escalates_and_outline_exhaustion_surfaces() {
        // Both flat PLAN attempts are invalid (4-file scope). There is no "flat-only
        // exhaustion" outcome anymore: the planner ALWAYS escalates into the
        // hierarchical OUTLINE/EXPAND/MERGE path once flat is exhausted. Here the
        // scripted model has nothing left for OUTLINE to consume either (its next
        // outputs are also the same invalid flat-shaped block, then the
        // `CapturingModel` "EXHAUSTED" sentinel), so OUTLINE itself exhausts its own
        // `MAX_OUTLINE_ATTEMPTS` retries — that OUTLINE-stage failure is what
        // surfaces, wrapped in the same top-level error, and the planner never
        // submits.
        let (_dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
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
        let err = run_planner("g", &model, &mcp, &fs, "proj", &noop_activity(), true)
            .await
            .expect_err("exhausted retries is an error");
        assert!(
            err.contains("could not produce a valid plan") && err.contains("OUTLINE"),
            "the escalation error must surface the OUTLINE-stage exhaustion, not a \
             generic flat-only message: {err}"
        );
        // It must NOT have submitted a plan.
        assert!(!mcp.call_names().iter().any(|n| n == "plan_submit"));

        // Prove escalation actually FIRED (not just a loosely-matching error string):
        // right after the 1 EXPLORE call + the 2 flat PLAN attempts, the NEXT model
        // call must be an OUTLINE prompt.
        let prompts = model.prompts();
        let outline_call_idx = 1 + FLAT_PLAN_ATTEMPTS;
        assert!(
            prompts.len() > outline_call_idx,
            "expected at least one OUTLINE call after 1 explore + {FLAT_PLAN_ATTEMPTS} flat \
             attempts, got {} prompt(s) total",
            prompts.len()
        );
        assert!(
            prompts[outline_call_idx].contains("milestone OUTLINE"),
            "the call right after the 2 flat attempts must be an OUTLINE prompt: {}",
            prompts[outline_call_idx]
        );
    }

    // --- Hierarchical escalation: run_planner integration --------------------

    fn outline_block(milestones: serde_json::Value) -> String {
        format!("```outline\n{}\n```", serde_json::json!({ "milestones": milestones }))
    }

    fn bad_scope_task() -> serde_json::Value {
        serde_json::json!([{
            "id": "T001", "title": "too big",
            "scope": ["a.rs", "b.rs", "c.rs", "d.rs"],
            "acceptance": "x", "dependsOn": [], "status": "pending", "attempts": 0,
        }])
    }

    fn fragment_task(id: &str, scope_file: &str) -> serde_json::Value {
        serde_json::json!([{
            "id": id, "title": format!("do {id}"), "scope": [scope_file],
            "acceptance": "cargo test", "dependsOn": [], "status": "pending", "attempts": 0,
        }])
    }

    #[tokio::test]
    async fn a_large_goal_that_succeeds_flat_on_attempt_one_never_escalates() {
        // No size-threshold pre-escalation: even a goal ~10x a typical one still takes
        // the flat path when it succeeds immediately — escalation is FAILURE-driven only.
        let (_dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs"], "approved");
        let model = CapturingModel::new(vec![note_block("src/a.rs"), plan_block(one_valid_task())]);
        let big_goal = "step ".repeat(2000);
        let outcome = run_planner(&big_goal, &model, &mcp, &fs, "proj", &noop_activity(), true)
            .await
            .expect("a large-but-successful goal still takes the flat path");
        assert_eq!(
            model.prompts().len(),
            2,
            "1 explore + 1 flat PLAN call — no outline/expand calls at all"
        );
        assert_eq!(outcome.tasks_plan.tasks.len(), 1);
    }

    #[tokio::test]
    async fn escalation_after_two_flat_failures_runs_outline_expand_merge_and_submits_once() {
        let (_dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs"], "approved");
        let outline_raw = outline_block(serde_json::json!([
            {"id": "M1", "title": "core", "files": [], "dependsOn": []},
            {"id": "M2", "title": "wiring", "files": [], "dependsOn": ["M1"]},
        ]));
        let model = CapturingModel::new(vec![
            note_block("src/a.rs"),                        // EXPLORE
            plan_block(bad_scope_task()),                   // flat attempt 1: rejected
            plan_block(bad_scope_task()),                   // flat attempt 2: rejected
            outline_raw,                                    // OUTLINE: accepted
            plan_block(fragment_task("T1", "core.rs")),     // EXPAND M1
            plan_block(fragment_task("T1", "wire.rs")),     // EXPAND M2
        ]);

        let outcome = run_planner(
            "build the thing",
            &model,
            &mcp,
            &fs,
            "proj",
            &noop_activity(),
            true,
        )
        .await
        .expect("hierarchical escalation succeeds");

        assert_eq!(outcome.tasks_plan.tasks.len(), 2, "one task per milestone, merged");
        let ids: Vec<&str> = outcome.tasks_plan.tasks.iter().map(|t| t.id.as_str()).collect();
        assert!(
            ids.contains(&"M1-T1") && ids.contains(&"M2-T1"),
            "namespaced ids: {ids:?}"
        );
        let m2 = outcome.tasks_plan.tasks.iter().find(|t| t.id == "M2-T1").unwrap();
        assert_eq!(
            m2.depends_on,
            vec!["M1-T1".to_string()],
            "cross-milestone ordering synthesized by the merge"
        );
        assert_eq!(
            mcp.call_names().iter().filter(|n| *n == "plan_submit").count(),
            1,
            "plan_submit fires exactly once"
        );
        let submit_pos = mcp.call_names().iter().position(|n| n == "plan_submit").unwrap();
        let create_pos = mcp
            .call_names()
            .iter()
            .position(|n| n == "project_create_plan_tasks")
            .unwrap();
        assert!(submit_pos < create_pos, "submit precedes the board create, as always");
    }

    #[tokio::test]
    async fn a_milestone_that_fails_both_expand_attempts_hard_errors_the_whole_plan() {
        let (_dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs"], "approved");
        let outline_raw =
            outline_block(serde_json::json!([{"id": "M1", "title": "core", "files": [], "dependsOn": []}]));
        let model = CapturingModel::new(vec![
            note_block("src/a.rs"),
            plan_block(bad_scope_task()), // flat 1 fails
            plan_block(bad_scope_task()), // flat 2 fails
            outline_raw,                  // outline accepted
            plan_block(bad_scope_task()), // EXPAND M1 attempt 1: also invalid
            plan_block(bad_scope_task()), // EXPAND M1 attempt 2: also invalid
        ]);
        let err = run_planner("g", &model, &mcp, &fs, "proj", &noop_activity(), true)
            .await
            .expect_err("a milestone failing both EXPAND attempts must hard-error the whole plan");
        assert!(err.contains("milestone M1"), "{err}");
        assert!(
            !mcp.call_names().iter().any(|n| n == "plan_submit"),
            "never submits a partial/broken plan"
        );
    }

    #[tokio::test]
    async fn outline_that_fails_twice_hard_errors_before_any_expand_call() {
        let (_dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs"], "approved");
        let model = CapturingModel::new(vec![
            note_block("src/a.rs"),
            plan_block(bad_scope_task()),
            plan_block(bad_scope_task()),
            "not an outline block".to_string(), // OUTLINE attempt 1: invalid
            "still not an outline".to_string(),  // OUTLINE attempt 2: invalid
        ]);
        let err = run_planner("g", &model, &mcp, &fs, "proj", &noop_activity(), true)
            .await
            .expect_err("outline exhaustion must hard-error");
        assert!(err.contains("hierarchical OUTLINE failed"), "{err}");
        assert_eq!(
            model.prompts().len(),
            5,
            "1 explore + 2 flat + 2 outline attempts, then stop (no EXPAND calls)"
        );
        assert!(!mcp.call_names().iter().any(|n| n == "plan_submit"));
    }

    #[tokio::test]
    async fn hierarchical_escalation_emits_outlining_and_expanding_milestones_in_order() {
        let (dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs"], "approved");
        let outline_raw = outline_block(serde_json::json!([
            {"id": "M1", "title": "core", "files": [], "dependsOn": []},
            {"id": "M2", "title": "wiring", "files": [], "dependsOn": ["M1"]},
        ]));
        let model = CapturingModel::new(vec![
            note_block("src/a.rs"),
            plan_block(bad_scope_task()),
            plan_block(bad_scope_task()),
            outline_raw,
            plan_block(fragment_task("T1", "core.rs")),
            plan_block(fragment_task("T1", "wire.rs")),
        ]);
        let act_file = dir.path().join("activity.jsonl");
        let activity = Activity::with_path(&act_file);

        run_planner("g", &model, &mcp, &fs, "proj", &activity, true)
            .await
            .expect("hierarchical escalation succeeds");

        let milestones = read_milestones(&act_file);
        let texts: Vec<&str> = milestones.iter().map(|(t, _)| t.as_str()).collect();
        let outline_idx = texts
            .iter()
            .position(|t| *t == "outlining plan (hierarchical)")
            .expect("outline milestone present");
        let m1_idx = texts
            .iter()
            .position(|t| *t == "expanding milestone M1 (1/2)")
            .expect("M1 expand milestone present");
        let m2_idx = texts
            .iter()
            .position(|t| *t == "expanding milestone M2 (2/2)")
            .expect("M2 expand milestone present");
        assert!(
            outline_idx < m1_idx && m1_idx < m2_idx,
            "outline then milestones IN ORDER: {texts:?}"
        );
        assert_eq!(milestones[outline_idx].1, "", "Hollow node (in-progress step)");
        assert_eq!(milestones[m1_idx].1, "");
        assert_eq!(milestones[m2_idx].1, "");
        assert!(
            texts.iter().any(|t| t.contains("drafted 2 tasks")),
            "the shared 'drafted N tasks' milestone still fires downstream: {texts:?}"
        );
    }

    #[tokio::test]
    async fn accepted_outline_is_appended_to_the_preplan_draft_outline_section() {
        let (dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        // "vanished" -> Timeout (non-terminal): the preplan file survives so we can
        // inspect it after the run, mirroring the existing Decisions-recording test.
        let mcp = MockMcp::new(vec!["src/a.rs"], "vanished");
        let outline_raw = outline_block(serde_json::json!([
            {"id": "M1", "title": "core", "files": [], "dependsOn": []},
            {"id": "M2", "title": "wiring", "files": [], "dependsOn": ["M1"]},
        ]));
        let model = CapturingModel::new(vec![
            note_block("src/a.rs"),
            plan_block(bad_scope_task()),
            plan_block(bad_scope_task()),
            outline_raw,
            plan_block(fragment_task("T1", "core.rs")),
            plan_block(fragment_task("T1", "wire.rs")),
        ]);
        run_planner("g", &model, &mcp, &fs, "proj", &noop_activity(), true)
            .await
            .expect("hierarchical escalation succeeds even when approval times out");

        let preplan_path = dir.path().join(".devboule").join("preplan.md");
        let body = std::fs::read_to_string(&preplan_path).expect("Timeout is non-terminal");
        assert!(body.contains("## Draft outline"));
        assert!(
            body.contains("- M1: core (depends on: none)"),
            "{body}"
        );
        assert!(
            body.contains("- M2: wiring (depends on: M1)"),
            "{body}"
        );
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
        assert_eq!(
            paths,
            vec!["src/good.rs", "src/also_good.rs"],
            "only safe paths kept: {paths:?}"
        );
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
        assert!(
            parse_structure(&text).is_err(),
            "all-unsafe spine is an error"
        );
    }

    #[test]
    fn plan_prompt_is_hard_bounded_for_giant_inputs() {
        // The local-model context guarantee: even a GIANT summary + goal + prior
        // error + preplan can never make the PLAN prompt exceed
        // MAX_PLAN_PROMPT_CHARS, exactly like the EXPLORE guard.
        let giant_goal = "step ".repeat(50_000); // ~250k chars
        let giant_preplan = "p".repeat(100_000);
        let giant_notes = "x".repeat(100_000);
        let giant_summary = serde_json::json!({ "blob": "y".repeat(100_000) });
        let giant_prior = "z".repeat(50_000);
        let prompt = build_plan_prompt(
            &giant_goal,
            &giant_preplan,
            &giant_notes,
            &giant_summary,
            Some(&giant_prior),
        );
        assert!(
            prompt.chars().count() <= MAX_PLAN_PROMPT_CHARS,
            "PLAN prompt must be hard-bounded: got {} chars (max {MAX_PLAN_PROMPT_CHARS})",
            prompt.chars().count()
        );
    }

    #[test]
    fn plan_prompt_fits_by_construction_with_all_parts_at_their_real_caps() {
        // Unlike the test above (which stress-tests the OUTER belt-and-suspenders
        // truncate against pathological, UNCAPPED inputs), this proves the
        // REALISTIC worst case — every part already at ITS OWN legitimate cap, as
        // `run_planner` always ensures before calling `build_plan_prompt` — fits
        // WITHOUT ever hitting the final truncate. If it didn't, that truncate
        // would silently cut the trailing RULES/schema text the model needs to
        // emit a valid ```plan``` block — exactly the failure mode adding the
        // PRE-PLAN NOTES section must not reintroduce.
        let goal = "s".repeat(MAX_GOAL_CHARS);
        let preplan_notes = "p".repeat(MAX_PREPLAN_PROMPT_CHARS);
        let notes = "n".repeat(MAX_NOTES_TOTAL_CHARS);
        // A bare JSON string serializes with 2 wrapping quotes; size the payload so
        // `summary.to_string()` lands EXACTLY at MAX_SUMMARY_CHARS (not over) — an
        // object wrapper (`{"blob":"..."}`) would add its OWN overhead and trip
        // THIS part's own (expected, pre-existing) truncate_chars, which is not
        // what this test is about (it tests the OUTER assembly, not per-part caps).
        let summary = serde_json::Value::String("y".repeat(MAX_SUMMARY_CHARS - 2));
        let prior_error = "e".repeat(MAX_PRIOR_ERROR_CHARS);
        let prompt = build_plan_prompt(&goal, &preplan_notes, &notes, &summary, Some(&prior_error));
        assert!(
            !prompt.contains("[…truncated]"),
            "the realistic worst case must fit WITHOUT truncation (schema tail preserved); \
             got {} chars (max {MAX_PLAN_PROMPT_CHARS})",
            prompt.chars().count()
        );
        assert!(
            prompt.trim_end().ends_with("```"),
            "the plan schema example must survive intact, got tail: {:?}",
            &prompt[prompt.len().saturating_sub(80)..]
        );
    }

    #[test]
    fn flat_plan_rules_block_is_byte_stable_plus_one_new_line() {
        // Touch-up: the RULES block gains ONE new line ("One task = one testable,
        // committable unit.") right after the scope-cap rule; everything else in the
        // block — content AND relative order — is byte-stable versus before this
        // feature, because `task_rules_block` is the SAME helper the flat prompt has
        // always effectively inlined (factored out, not rewritten).
        let prompt = build_plan_prompt("g", "", "(no notes)", &serde_json::json!({}), None);
        let scope_idx = prompt
            .find(&format!(
                "- Each task `scope` (files it MODIFIES) has AT MOST {MAX_TASK_SCOPE} entries"
            ))
            .expect("scope rule present");
        let one_task_idx = prompt
            .find("- One task = one testable, committable unit.\n")
            .expect("the new rule is present");
        let acceptance_idx = prompt
            .find("- `acceptance` MUST be a deterministically verifiable check")
            .expect("acceptance rule present");
        let ids_idx = prompt
            .find("- Task `id`s are unique and non-empty")
            .expect("id rule present");
        let paths_idx = prompt
            .find("- All paths are project-root-relative")
            .expect("paths rule present");
        let max_tasks_idx = prompt
            .find(&format!("- At most {MAX_TASKS} tasks.\n"))
            .expect("max-tasks rule present (flat only)");
        let status_idx = prompt
            .find("- Every task starts with \"status\": \"pending\" and \"attempts\": 0.")
            .expect("status/attempts rule present");
        let weight_idx = prompt
            .find("- Optional \"weight\": \"main\" routes the task")
            .expect("weight rule present");
        assert!(
            scope_idx < one_task_idx
                && one_task_idx < acceptance_idx
                && acceptance_idx < ids_idx
                && ids_idx < paths_idx
                && paths_idx < max_tasks_idx
                && max_tasks_idx < status_idx
                && status_idx < weight_idx,
            "RULES block must keep the ORIGINAL order with the new line inserted right \
             after the scope rule: {prompt}"
        );
    }

    // --- Hierarchical escalation: OUTLINE validation + MERGE (the merge test matrix) ---

    /// A minimal fragment [`Task`] with `id` and `depends_on` (LOCAL ids), one scope
    /// file matching `id`, and a valid acceptance — everything else at a passing
    /// default.
    fn frag_task(id: &str, deps: &[&str]) -> Task {
        Task {
            id: id.into(),
            title: format!("do {id}"),
            scope: vec![format!("{id}.rs")],
            context_files: vec![],
            acceptance: "cargo test".into(),
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            status: "pending".into(),
            attempts: 0,
            weight: String::new(),
        }
    }

    fn frag(tasks: Vec<Task>) -> TasksPlan {
        TasksPlan {
            project_goal: "fragment goal".into(),
            tasks,
        }
    }

    fn mk_milestone(id: &str, deps: &[&str]) -> Milestone {
        Milestone {
            id: id.into(),
            title: format!("milestone {id}"),
            files: vec![],
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn mk_outline(milestones: Vec<Milestone>) -> Outline {
        Outline { milestones }
    }

    #[test]
    fn merge_single_milestone_namespaces_ids_with_no_extra_deps() {
        let outline = mk_outline(vec![mk_milestone("M1", &[])]);
        let merged = merge_fragments("g", &outline, &[frag(vec![frag_task("T001", &[])])])
            .expect("single milestone merges");
        assert_eq!(merged.tasks.len(), 1);
        assert_eq!(merged.tasks[0].id, "M1-T001");
        assert!(merged.tasks[0].depends_on.is_empty());
        assert_eq!(merged.project_goal, "g");
    }

    #[test]
    fn merge_chain_wires_dependents_root_to_prerequisites_leaf_and_namespaces_duplicate_local_ids() {
        // M2 dependsOn M1; BOTH fragments locally use the SAME id "T001" — proving
        // namespacing prevents a collision after merge.
        let outline = mk_outline(vec![mk_milestone("M1", &[]), mk_milestone("M2", &["M1"])]);
        let f1 = frag(vec![frag_task("T001", &[])]);
        let f2 = frag(vec![frag_task("T001", &[])]);
        let merged = merge_fragments("g", &outline, &[f1, f2]).expect("chain merges");
        assert_eq!(merged.tasks.len(), 2, "both local T001s survive, namespaced distinctly");
        let m1 = merged.tasks.iter().find(|t| t.id == "M1-T001").unwrap();
        assert!(m1.depends_on.is_empty());
        let m2 = merged.tasks.iter().find(|t| t.id == "M2-T001").unwrap();
        assert_eq!(
            m2.depends_on,
            vec!["M1-T001".to_string()],
            "M2's root picks up M1's leaf as a dependency"
        );
    }

    #[test]
    fn merge_diamond_unions_leaves_from_multiple_prerequisites() {
        // M1 <- M2, M1 <- M3, M4 depends on BOTH M2 and M3.
        let outline = mk_outline(vec![
            mk_milestone("M1", &[]),
            mk_milestone("M2", &["M1"]),
            mk_milestone("M3", &["M1"]),
            mk_milestone("M4", &["M2", "M3"]),
        ]);
        let fragments = vec![
            frag(vec![frag_task("A", &[])]),
            frag(vec![frag_task("A", &[])]),
            frag(vec![frag_task("A", &[])]),
            frag(vec![frag_task("A", &[])]),
        ];
        let merged = merge_fragments("g", &outline, &fragments).expect("diamond merges");
        let m2 = merged.tasks.iter().find(|t| t.id == "M2-A").unwrap();
        assert_eq!(m2.depends_on, vec!["M1-A".to_string()]);
        let m3 = merged.tasks.iter().find(|t| t.id == "M3-A").unwrap();
        assert_eq!(m3.depends_on, vec!["M1-A".to_string()]);
        let m4 = merged.tasks.iter().find(|t| t.id == "M4-A").unwrap();
        let mut deps = m4.depends_on.clone();
        deps.sort();
        assert_eq!(
            deps,
            vec!["M2-A".to_string(), "M3-A".to_string()],
            "M4's root unions BOTH prerequisites' leaves"
        );
    }

    #[test]
    fn merge_identifies_roots_and_leaves_of_a_fragments_internal_dag() {
        // M1 is a linear chain T1 -> T2 -> T3: root=T1 only, leaf=T3 only. M2 (which
        // depends on M1) must be wired to M1's LEAF (T3), never the middle/root task.
        let outline = mk_outline(vec![mk_milestone("M1", &[]), mk_milestone("M2", &["M1"])]);
        let f1 = frag(vec![
            frag_task("T1", &[]),
            frag_task("T2", &["T1"]),
            frag_task("T3", &["T2"]),
        ]);
        let f2 = frag(vec![frag_task("X", &[])]);
        let merged = merge_fragments("g", &outline, &[f1, f2]).expect("merges");
        let x = merged.tasks.iter().find(|t| t.id == "M2-X").unwrap();
        assert_eq!(
            x.depends_on,
            vec!["M1-T3".to_string()],
            "only the fragment's LEAF (T3) is wired in, not T1/T2"
        );
    }

    #[test]
    fn fragment_roots_and_leaves_identify_a_linear_chains_ends() {
        let tasks = vec![
            frag_task("T1", &[]),
            frag_task("T2", &["T1"]),
            frag_task("T3", &["T2"]),
        ];
        assert_eq!(fragment_roots(&tasks), vec!["T1"]);
        assert_eq!(fragment_leaves(&tasks), vec!["T3"]);
    }

    #[test]
    fn fragment_roots_and_leaves_handle_a_single_task_fragment() {
        let tasks = vec![frag_task("T1", &[])];
        assert_eq!(fragment_roots(&tasks), vec!["T1"]);
        assert_eq!(fragment_leaves(&tasks), vec!["T1"]);
    }

    #[test]
    fn merge_rejects_a_dangling_intra_fragment_dependency() {
        let outline = mk_outline(vec![mk_milestone("M1", &[])]);
        let f1 = frag(vec![frag_task("T001", &["T999"])]); // T999 not in this fragment
        let err = merge_fragments("g", &outline, &[f1]).expect_err("dangling local dep errors");
        assert!(
            err.contains("M1") && err.contains("T999") && err.contains("local"),
            "{err}"
        );
    }

    #[test]
    fn merge_over_max_tasks_errors_with_hierarchical_hint() {
        let n_milestones = 5usize;
        let tasks_per = MAX_TASKS / n_milestones + 2; // total > MAX_TASKS
        let mut milestones = Vec::new();
        let mut fragments = Vec::new();
        for mi in 0..n_milestones {
            milestones.push(mk_milestone(&format!("M{mi}"), &[]));
            let tasks: Vec<Task> = (0..tasks_per)
                .map(|ti| frag_task(&format!("T{ti}"), &[]))
                .collect();
            fragments.push(frag(tasks));
        }
        let outline = mk_outline(milestones);
        let total = n_milestones * tasks_per;
        let err = merge_fragments("g", &outline, &fragments)
            .expect_err("an over-max merged plan must error");
        assert!(err.contains("hierarchical merge produced"), "{err}");
        assert!(err.contains(&total.to_string()), "{err}");
        assert!(err.contains(&MAX_TASKS.to_string()), "{err}");
    }

    #[test]
    fn merge_rejects_mismatched_milestone_and_fragment_counts() {
        let outline = mk_outline(vec![mk_milestone("M1", &[]), mk_milestone("M2", &["M1"])]);
        let err = merge_fragments("g", &outline, &[frag(vec![frag_task("T001", &[])])])
            .expect_err("a count mismatch must error");
        assert!(err.contains("1:1"), "{err}");
    }

    #[test]
    fn merge_normalizes_runtime_fields_a_model_echoing_done_is_forced_back_to_pending() {
        // The harness OWNS runtime fields (status/attempts), never the model. An
        // EXPAND fragment task that echoes status:"done"/attempts:3 (e.g. a model
        // that copy-pasted a completed task from its own training data, or just
        // hallucinated) must land in the MERGED plan as pending/0 — never poisoning
        // the freshly-created board with a task that looks already-finished.
        let outline = mk_outline(vec![mk_milestone("M1", &[])]);
        let mut task = frag_task("T001", &[]);
        task.status = "done".into();
        task.attempts = 3;
        let merged = merge_fragments("g", &outline, &[frag(vec![task])]).expect("merges");
        assert_eq!(merged.tasks.len(), 1);
        assert_eq!(merged.tasks[0].status, "pending");
        assert_eq!(merged.tasks[0].attempts, 0);
    }

    #[test]
    fn merge_preserves_weight_and_context_files_across_namespacing() {
        // The Phase-11.3 runner routes execution on `weight` ("main" vs "mini") and
        // reads `contextFiles` for read-only deps — a silent loss of either during
        // namespacing would break main-coder routing / starve a task of its
        // declared context, with no validator to catch it (both fields are
        // orthogonal to the id/dependsOn remap this pass performs).
        let outline = mk_outline(vec![mk_milestone("M1", &[])]);
        let mut task = frag_task("T001", &[]);
        task.weight = "main".into();
        task.context_files = vec!["src/shared.rs".into()];
        let merged = merge_fragments("g", &outline, &[frag(vec![task])]).expect("merges");
        assert_eq!(merged.tasks.len(), 1);
        assert_eq!(merged.tasks[0].weight, "main");
        assert_eq!(merged.tasks[0].context_files, vec!["src/shared.rs".to_string()]);
    }

    #[test]
    fn validate_outline_rejects_a_milestone_cycle() {
        let outline = mk_outline(vec![mk_milestone("M1", &["M2"]), mk_milestone("M2", &["M1"])]);
        let err = validate_outline(&outline).expect_err("a milestone cycle must be rejected");
        assert!(err.contains("cycle"), "{err}");
    }

    #[test]
    fn validate_outline_caps_at_max_milestones() {
        let outline = mk_outline(
            (0..(MAX_MILESTONES + 1))
                .map(|i| mk_milestone(&format!("M{i}"), &[]))
                .collect(),
        );
        let err = validate_outline(&outline).expect_err("too many milestones must be rejected");
        assert!(err.contains("too many milestones"), "{err}");
    }

    #[test]
    fn validate_outline_rejects_duplicate_ids() {
        let outline = mk_outline(vec![mk_milestone("M1", &[]), mk_milestone("M1", &[])]);
        let err = validate_outline(&outline).expect_err("duplicate milestone id must be rejected");
        assert!(err.contains("duplicate"), "{err}");
    }

    #[test]
    fn validate_outline_rejects_dangling_dep() {
        let outline = mk_outline(vec![mk_milestone("M1", &["M999"])]);
        let err = validate_outline(&outline).expect_err("dangling milestone dep must be rejected");
        assert!(err.contains("unknown milestone id"), "{err}");
    }

    #[test]
    fn validate_outline_rejects_self_dep() {
        let outline = mk_outline(vec![mk_milestone("M1", &["M1"])]);
        let err = validate_outline(&outline).expect_err("a self-dep must be rejected");
        assert!(err.contains("itself"), "{err}");
    }

    #[test]
    fn validate_outline_rejects_empty_milestones() {
        let outline = mk_outline(vec![]);
        let err = validate_outline(&outline).expect_err("an empty outline must be rejected");
        assert!(err.contains("no milestones"), "{err}");
    }

    #[test]
    fn parse_outline_extracts_and_filters_unsafe_file_hints() {
        let raw = format!(
            "```outline\n{}\n```",
            serde_json::json!({
                "milestones": [
                    {"id": "M1", "title": "do the core work",
                     "files": ["src/good.rs", "../escape.rs", "/etc/passwd"], "dependsOn": []}
                ]
            })
        );
        let outline = parse_outline(&raw).expect("a valid outline parses");
        assert_eq!(
            outline.milestones[0].files,
            vec!["src/good.rs".to_string()],
            "unsafe file hints are dropped, not fatal"
        );
    }

    #[test]
    fn parse_outline_requires_exactly_one_block() {
        let block = format!(
            "```outline\n{}\n```",
            serde_json::json!({"milestones": [{"id": "M1", "title": "t", "files": [], "dependsOn": []}]})
        );
        assert!(parse_outline("no block here").is_err());
        let two = format!("{block}\n{block}");
        assert!(parse_outline(&two).is_err());
        assert!(parse_outline(&block).is_ok());
    }

    #[test]
    fn build_outline_prompt_carries_goal_failures_and_is_bounded() {
        let summary = serde_json::json!({"scanned": 3});
        let prompt = build_outline_prompt(
            "build the thing",
            "(no notes)",
            &summary,
            "scope has 4 files (max 3)",
            None,
        );
        assert!(prompt.contains("build the thing"), "{prompt}");
        assert!(
            prompt.contains("The flat plan failed twice: scope has 4 files (max 3)"),
            "{prompt}"
        );
        assert!(prompt.contains("```outline"), "{prompt}");
        assert!(prompt.chars().count() <= MAX_PLAN_PROMPT_CHARS);
    }

    #[test]
    fn build_outline_prompt_retry_feeds_back_the_precise_error() {
        let prompt = build_outline_prompt(
            "g",
            "",
            &serde_json::json!({}),
            "flat failure",
            Some("duplicate milestone id `M1`"),
        );
        assert!(
            prompt.contains("YOUR PREVIOUS OUTLINE WAS REJECTED")
                && prompt.contains("duplicate milestone id"),
            "{prompt}"
        );
    }

    #[test]
    fn build_outline_prompt_is_hard_bounded_for_giant_inputs() {
        let giant_goal = "step ".repeat(50_000);
        let giant_preplan = "p".repeat(100_000);
        let giant_summary = serde_json::json!({"blob": "y".repeat(100_000)});
        let giant_failures = "z".repeat(50_000);
        let prompt =
            build_outline_prompt(&giant_goal, &giant_preplan, &giant_summary, &giant_failures, None);
        assert!(prompt.chars().count() <= MAX_PLAN_PROMPT_CHARS);
    }

    #[test]
    fn build_expand_prompt_carries_milestone_framing_and_shares_the_task_rules() {
        let m = mk_milestone("M1", &[]);
        let m = Milestone {
            title: "wire the auth module".into(),
            files: vec!["src/auth.rs".into()],
            ..m
        };
        let prompt = build_expand_prompt("build the thing", "(notes)", &m, None);
        assert!(prompt.contains("build the thing"), "{prompt}");
        assert!(prompt.contains("MILESTONE M1: wire the auth module"), "{prompt}");
        assert!(prompt.contains("src/auth.rs"), "{prompt}");
        assert!(
            prompt.contains("ids in THIS fragment are LOCAL"),
            "the fragment-local-id rule is present: {prompt}"
        );
        // The SAME shared RULES text as the flat prompt (touch-up 6a: proves no drift).
        assert!(
            prompt.contains("- One task = one testable, committable unit.\n"),
            "{prompt}"
        );
        assert!(prompt.contains("```plan"), "{prompt}");
        assert!(prompt.chars().count() <= MAX_PLAN_PROMPT_CHARS);
    }

    #[test]
    fn build_expand_prompt_with_no_files_says_so() {
        let m = mk_milestone("M1", &[]);
        let prompt = build_expand_prompt("g", "", &m, None);
        assert!(prompt.contains("(none suggested)"), "{prompt}");
    }

    #[test]
    fn parse_expand_fragment_accepts_a_valid_fragment_and_rejects_an_invalid_one() {
        let good = plan_block(one_valid_task());
        assert!(parse_expand_fragment(&good, "M1").is_ok());
        let bad = plan_block(serde_json::json!([{
            "id": "T001", "title": "too big",
            "scope": ["a.rs", "b.rs", "c.rs", "d.rs"],
            "acceptance": "x", "dependsOn": [], "status": "pending", "attempts": 0,
        }]));
        let err = parse_expand_fragment(&bad, "M1").expect_err("scope > 3 is rejected");
        assert!(err.contains("milestone M1") && err.contains("scope"), "{err}");
    }

    #[test]
    fn parse_expand_fragment_rejects_a_dep_outside_the_fragment_as_an_unknown_id() {
        // A fragment task dependsOn an id NOT emitted in this same fragment is caught
        // by the reused validate_plan_structure (it only knows this fragment's ids),
        // giving fragment-local-id enforcement "for free".
        let raw = plan_block(serde_json::json!([{
            "id": "T001", "title": "t", "scope": ["a.rs"], "acceptance": "x",
            "dependsOn": ["T999"], "status": "pending", "attempts": 0,
        }]));
        let err = parse_expand_fragment(&raw, "M2").expect_err("dangling local dep rejected");
        assert!(err.contains("milestone M2") && err.contains("unknown task id"), "{err}");
    }

    #[test]
    fn parse_structure_non_json_is_error() {
        assert!(parse_structure("not json").is_err());
    }

    #[test]
    fn missing_project_id_escalates() {
        // Validated synchronously before any tool call.
        let plan = TasksPlan {
            project_goal: "g".into(),
            tasks: vec![],
        };
        // (use validate to exercise empty-tasks path too)
        assert!(validate_plan(&plan).is_err());
    }

    #[test]
    fn extract_one_block_requires_exactly_one() {
        assert!(extract_one_block("no block here", "plan").is_err());
        let two = format!(
            "{}\n{}",
            plan_block(one_valid_task()),
            plan_block(one_valid_task())
        );
        assert!(extract_one_block(&two, "plan").is_err());
        assert!(extract_one_block(&plan_block(one_valid_task()), "plan").is_ok());
    }

    #[test]
    fn approval_from_status_is_case_insensitive_and_conservative() {
        assert_eq!(
            PlanApproval::from_status("APPROVED"),
            PlanApproval::Approved
        );
        assert_eq!(
            PlanApproval::from_status(" rejected "),
            PlanApproval::Rejected
        );
        assert_eq!(
            PlanApproval::from_status("pending_approval"),
            PlanApproval::Timeout
        );
        assert_eq!(PlanApproval::from_status("whatever"), PlanApproval::Timeout);
    }

    // P2: whitespace-only planId must be treated as absent.
    #[test]
    fn whitespace_only_plan_id_is_treated_as_absent() {
        // A server that returns `" "` (spaces only) for planId must not be forwarded
        // to `project_create_plan_tasks` as a blank tag — it must map to None.
        let result = parse_submit_result(
            &serde_json::json!({"planId": "  ", "status": "approved"}).to_string(),
        );
        assert_eq!(
            result.plan_id, None,
            "a whitespace-only planId must be treated as absent"
        );
        assert_eq!(result.approval, PlanApproval::Approved);
    }

    #[test]
    fn non_empty_plan_id_is_preserved_after_trim() {
        // A planId with surrounding whitespace is trimmed but retained.
        let result = parse_submit_result(
            &serde_json::json!({"planId": "  p1  ", "status": "approved"}).to_string(),
        );
        assert_eq!(
            result.plan_id.as_deref(),
            Some("p1"),
            "a non-empty planId is trimmed and kept"
        );
    }

    #[tokio::test]
    async fn empty_goal_escalates() {
        let (_dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs"], "approved");
        let model = CapturingModel::new(vec![]);
        let err = run_planner("   ", &model, &mcp, &fs, "proj", &noop_activity(), true)
            .await
            .expect_err("an empty goal is rejected before any work");
        assert!(err.contains("non-empty goal"));
        // No tool calls at all.
        assert!(mcp.call_names().is_empty());
    }

    #[tokio::test]
    async fn empty_project_id_escalates() {
        let (_dir, fs) = fs_with_files(&[("src/a.rs", "fn a() {}\n")]);
        let mcp = MockMcp::new(vec!["src/a.rs"], "approved");
        let model = CapturingModel::new(vec![]);
        let err = run_planner("g", &model, &mcp, &fs, "", &noop_activity(), true)
            .await
            .expect_err("an empty project_id is rejected before any work");
        assert!(err.contains("project_id"));
        assert!(mcp.call_names().is_empty());
    }
}
