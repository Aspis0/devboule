//! Phase 11.3 / R7 — the deterministic CONCURRENT DAG runner, over the shared KANBAN.
//!
//! Reads the active plan's tasks straight off the project Kanban (the single shared
//! task store — Phase 11.5-B Piece 1; `.devboule/tasks.json` is GONE) and runs them in
//! dependency order, delegating each to the `spawn_mini_coder` MCP tool (which already
//! carries the deterministic Censor gate + the mini's retry/escalate chain). R7: each
//! iteration runs the whole READY BATCH (all tasks whose deps are satisfied) up to
//! `MAX_PARALLEL_TASKS` CONCURRENTLY — independent tasks (e.g. a diamond's two arms) run
//! in parallel; a linear chain still runs serially (one ready at a time). When any task in
//! a batch does not complete cleanly the runner finishes the in-flight batch (every Done
//! sibling still lands in `review`), then STOPS and reports the first block so the
//! orchestrator can `ask_user` rather than blindly retrying.
//!
//! DETERMINISTIC: there is NO LLM here. The planner already produced the plan, the human
//! already approved it (`plan_submit` gate), and the approved plan's tasks were created
//! on the board (`project_create_plan_tasks`). The runner just drives them. It needs only
//! an [`McpBackend`] (to read + advance the board, and delegate), the Oracle-side
//! `project_id`, and the [`Activity`] milestone emitter (Console observability).
//!
//! SHARED-BOARD AWARE: the Kanban is now the source of truth and CONCURRENTLY mutable (a
//! human or Claude can move a card mid-run). So the runner re-reads `project_get` at the
//! START of every iteration and never caches the task list across iterations. The status
//! vocabulary is the Kanban's (`todo|wip|review|blocked|done`); `done` is VERIFIER-ONLY,
//! so a finished task is set to **`review`** (the human/verifier gate stays), never `done`.
//!
//! ATTEMPTS: the Kanban task has NO `attempts` field, so the cross-run attempt counter is
//! gone. We track re-attempts of an interrupted (`wip`) task IN-RUN (a local map keyed by
//! task id) and cap them at [`MAX_TASK_ATTEMPTS`]; combined with the belt-and-suspenders
//! iteration bound and the stall/blocked detection, this bounds the run without persisting
//! a counter on the board.

use std::collections::HashMap;
use std::collections::HashSet;

use crate::activity::{Activity, Node};
use crate::executor::McpBackend;
use futures::StreamExt;

/// Max times the runner will delegate a SINGLE task WITHIN ONE RUN before giving up and
/// marking it `blocked`. The check fires BEFORE the (MAX_TASK_ATTEMPTS+1)th delegation,
/// so exactly this many delegations occur before the task is blocked. A task already `wip`
/// when the run starts (an interrupted prior run, or a human moving a card) is
/// re-attemptable; the IN-RUN counter is keyed by task id and checked before each
/// delegation — when it reaches this cap the task is blocked rather than re-run,
/// preventing an unbounded loop on a poison task. The counter is NOT persisted (the Kanban
/// has no `attempts` field); a brand-new run starts every task's counter at zero, which is
/// correct: a fresh run is a fresh human decision to proceed.
pub const MAX_TASK_ATTEMPTS: u32 = 2;

/// Cap on the composed `task` string handed to `spawn_mini_coder`. The server caps the
/// task length too; we cap here (char-wise, never splitting a codepoint) so a verbose
/// title+acceptance can never overflow the delegated directive.
const MAX_DELEGATED_TASK_CHARS: usize = 4_200;

/// The Kanban task-status vocabulary. `done` is VERIFIER-ONLY (the runner never sets it);
/// the runner advances a task `todo|wip` → `wip` (claim) → `review` (mini done) or
/// `blocked` (mini did not complete / exhausted in-run attempts). A `review`/`done` task
/// is terminal for the runner; a `blocked` task is NOT auto-re-run.
const STATUS_TODO: &str = "todo";
const STATUS_WIP: &str = "wip";
const STATUS_REVIEW: &str = "review";
const STATUS_BLOCKED: &str = "blocked";
const STATUS_DONE: &str = "done";

/// Evidence string attached to a runner-driven `project_update_status`. The server
/// A task that stopped the run, with enough detail for the orchestrator to `ask_user`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedTask {
    pub id: String,
    pub title: String,
    /// The task's status at the block (`blocked`), or the predecessor's status in a stall.
    pub status: String,
    /// A short, human-facing reason (the mini's terminal status + capped error, or the
    /// dependency-stall explanation).
    pub reason: String,
}

/// What one `run_tasks` returned: how many plan tasks are FINISHED (in `review` or
/// `done`), the total in the active plan, and the block (if the run stopped on one).
/// `blocked == None` means every plan task is finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunReport {
    pub completed: usize,
    pub total: usize,
    pub blocked: Option<BlockedTask>,
}

impl RunReport {
    /// The compact line the executor feeds back to the burst model (mirrors the
    /// planner's `compact_summary`): names the progress and, on a block, the task +
    /// the ask-the-user directive so the orchestrator does not silently retry. "Done"
    /// here means FINISHED — `review` or `done` — since the runner lands tasks in
    /// `review` for the human/verifier gate, never `done` itself.
    pub fn compact_summary(&self) -> String {
        match &self.blocked {
            None => format!(
                "Ran {}/{} plan task(s); all finished (in review/done).",
                self.completed, self.total
            ),
            Some(b) => format!(
                "Ran {}/{} plan task(s); BLOCKED on {} '{}' ({}). Ask the user how to proceed.",
                self.completed,
                self.total,
                b.id,
                elide(&b.title),
                b.reason
            ),
        }
    }
}

/// The mini's per-task verdict, reduced to the only distinction the runner acts on:
/// `done` (proceed) vs everything-else (block, with a reason). When done, carries the
/// mini's output text so the review evidence can reflect the real outcome.
enum MiniVerdict {
    Done(String),
    Blocked(String),
}

/// A compact, owned VIEW of one Kanban task, built fresh from a `project_get` response
/// each iteration. We never cache the live task objects across iterations (the board is
/// shared + concurrently mutable); this view is rebuilt every loop turn from the latest
/// read so the Kanban stays the source of truth.
#[derive(Debug, Clone)]
struct TaskView {
    id: String,
    title: String,
    status: String,
    depends_on: Vec<String>,
    scope: Vec<String>,
    acceptance: String,
    /// The task's `planId` tag (the active plan it belongs to). Always present here (we
    /// only build views for plan-tagged tasks).
    plan_id: String,
    /// The task's `updatedAt` (raw ISO string), used ONLY to break a tie between multiple
    /// concurrently-active plans. Lexicographic max of ISO-8601 timestamps == most recent.
    updated_at: String,
    /// The task's persisted review evidence (the mini's real output+files), read back
    /// from the board. Empty when the task has no evidence yet. Used to feed a task's
    /// completed-dependency context even across a fresh `run_tasks` re-invocation.
    evidence: String,
    /// ROLE UNTANGLE Phase 4: the task's execution tier — "main" routes the
    /// dispatch to `spawn_main_coder`; anything else (absent/empty/"mini") stays
    /// on `spawn_mini_coder`.
    weight: String,
}

/// The next thing the linear runner should do given the active plan's current state.
enum Frontier {
    /// Every active-plan task is finished (`review` or `done`).
    AllDone,
    /// This task index (into the active-plan view list) is `todo`/`wip` with all deps
    /// satisfied (`review`/`done`) — run it.
    Ready(usize),
    /// Non-finished tasks remain but NONE is ready (they wait on a `blocked`/incomplete
    /// predecessor — acyclicity rules out a circular wait). Carries the index to name in
    /// the block report (the blocked root cause when one exists, else the first waiter).
    Stalled(usize),
}

/// Run the active plan to completion or the first block, against the shared Kanban.
///
/// Reads the board via `project_get`, auto-detects the active plan (the planId group with
/// ≥1 unfinished task), then drives the linear DAG: re-read the board, pick a ready task,
/// claim it to `wip`, delegate it to `spawn_mini_coder`, set it `review` on success (the
/// human/verifier gate sets `done`), and continue until every plan task is finished or one
/// blocks. Returns a [`RunReport`]; an `Err` is reserved for a transport/read failure or
/// no plan to run (a *blocked task* is a successful `Ok(report)` whose `blocked` is set —
/// an expected outcome, not an error).
pub async fn run_tasks(
    mcp: &dyn McpBackend,
    project_id: &str,
    activity: &Activity,
) -> Result<RunReport, String> {
    if project_id.trim().is_empty() {
        return Err("runner needs a project_id (DEVBOULE_PROJECT_ID not set?)".to_string());
    }

    let first = read_plan_views(mcp, project_id).await?;
    let active_plan_id = match select_active_plan(&first) {
        Some(id) => id,
        None => {
            if !has_plan_tasks(&first) {
                return Err(format!(
                    "no active plan on the board for project '{project_id}'; run `plan` first"
                ));
            }
            let finished = first
                .iter()
                .filter(|t| t.status == STATUS_REVIEW || t.status == STATUS_DONE)
                .count();
            let total = first.len();
            return Ok(RunReport {
                completed: finished,
                total,
                blocked: None,
            });
        }
    };
    activity.milestone(
        &format!("running plan {}", elide(&active_plan_id)),
        Node::Dot,
    );

    let mut attempts: HashMap<String, u32> = HashMap::new();
    // v6 Phase 4 (SUMMARY-feeding): completed-task summaries recorded IN THIS RUN so a
    // dependent task's spawn can carry what its dependencies actually produced. In-memory
    // only — a dep completed in a PRIOR run (pre-restart) simply yields no summary
    // (graceful; cross-run persistence is a separate concern).
    let mut summaries: HashMap<String, String> = HashMap::new();
    const MAX_ITER_BOUND: usize = crate::planner::MAX_TASKS
        .saturating_mul(MAX_TASK_ATTEMPTS as usize + 1)
        .saturating_add(1);
    const MAX_PARALLEL_TASKS: usize = 2;

    for _ in 0..MAX_ITER_BOUND {
        let views = read_plan_views(mcp, project_id).await?;
        let plan: Vec<TaskView> = views
            .into_iter()
            .filter(|t| t.plan_id == active_plan_id)
            .collect();

        let completed = count_finished(&plan);
        let total = plan.len();

        let batch = ready_batch(&plan);
        if batch.is_empty() {
            match next_runnable(&plan) {
                Frontier::AllDone => {
                    return Ok(RunReport {
                        completed,
                        total,
                        blocked: None,
                    });
                }
                Frontier::Stalled(idx) => {
                    let t = &plan[idx];
                    let reason = match t.status.as_str() {
                        STATUS_BLOCKED => "previously blocked".to_string(),
                        STATUS_WIP => {
                            "interrupted (left wip) and depends on a task that did not finish"
                                .to_string()
                        }
                        _ => "depends on a task that did not finish".to_string(),
                    };
                    let blocked = BlockedTask {
                        id: t.id.clone(),
                        title: t.title.clone(),
                        status: t.status.clone(),
                        reason,
                    };
                    return Ok(RunReport {
                        completed,
                        total,
                        blocked: Some(blocked),
                    });
                }
                Frontier::Ready(_) => unreachable!("batch is empty but next_runnable says Ready"),
            }
        }

        // B2: SELECT up to MAX_PARALLEL_TASKS ready tasks. Do NOT touch the attempt counter
        // here — only READ it for the cap check; we charge an attempt below, ONLY for tasks we
        // actually dispatch, so a task that is selected-but-never-run (the batch cut short by
        // an over-cap sibling) is never charged an attempt it didn't use.
        let mut selected_indices: Vec<usize> = Vec::new();
        let mut blocked_report: Option<RunReport> = None;
        for &idx in &batch {
            let task = &plan[idx];
            let count = *attempts.get(&task.id).unwrap_or(&0);
            if count >= MAX_TASK_ATTEMPTS {
                let reason =
                    format!("exceeded {MAX_TASK_ATTEMPTS} in-run attempts without finishing");
                // W4: best-effort (consistent with the post-batch block path) — a failed board
                // write must surface the block reason, not abort the whole run with a transport
                // Err, and must report the board's REAL status.
                let (status, reason) =
                    block_best_effort(mcp, project_id, activity, &task.id, reason).await;
                if status == STATUS_BLOCKED {
                    activity.milestone(&format!("blocked {}", task.id), Node::Terra);
                }
                blocked_report = Some(RunReport {
                    completed,
                    total,
                    blocked: Some(BlockedTask {
                        id: task.id.clone(),
                        title: task.title.clone(),
                        status,
                        reason,
                    }),
                });
                break;
            }
            // v6 Phase 5 (isolation): never run two tasks with OVERLAPPING scope in the
            // SAME parallel batch — two minis writing the same file would corrupt each
            // other's edits (the minis share one working tree). Defer this task to a later
            // batch; it stays ready and runs once the conflicting sibling has finished.
            if selected_indices
                .iter()
                .any(|&s| scopes_overlap(plan[s].scope.as_slice(), task.scope.as_slice()))
            {
                continue;
            }
            selected_indices.push(idx);
            if selected_indices.len() >= MAX_PARALLEL_TASKS {
                break;
            }
        }

        if let Some(report) = blocked_report {
            return Ok(report);
        }

        // B2: charge exactly one attempt per task we are about to dispatch (now that the
        // selection is final and no over-cap sibling cut it short).
        for &idx in &selected_indices {
            *attempts.entry(plan[idx].id.clone()).or_insert(0) += 1;
        }

        // Run the selected ready tasks CONCURRENTLY (bounded by MAX_PARALLEL_TASKS). Each
        // future OWNS a cloned TaskView (an `async move` borrowing &plan[idx] would move the
        // whole `plan` Vec into the closure, and we still need `plan` afterwards); TaskView is
        // Clone + cheap. mcp/activity are shared refs (Send+Sync), copied into each future. Sort
        // by idx so the post-batch processing + any block report is deterministic.
        let selected: Vec<(usize, TaskView, Vec<(String, String)>)> = selected_indices
            .iter()
            .map(|&idx| {
                let t = plan[idx].clone();
                // Gather the summaries of this task's already-completed dependencies (they
                // ran in a prior iteration; batch siblings never depend on each other).
                // If the in-memory `summaries` map has NO entry for a dependency, fall back to
                // that dependency's `evidence` field from the current `plan` views (the board
                // is authoritative and survives re-invocation).
                let deps: Vec<(String, String)> = t
                    .depends_on
                    .iter()
                    .filter_map(|d| {
                        let s = summaries
                            .get(d)
                            .cloned()
                            .or_else(|| {
                                plan.iter()
                                    .find(|v| &v.id == d)
                                    .map(|v| v.evidence.clone())
                                    .filter(|e| !e.trim().is_empty())
                            });
                        s.map(|s| (d.clone(), s))
                    })
                    .collect();
                (idx, t, deps)
            })
            .collect();
        let mut results: Vec<(usize, MiniVerdict)> = futures::stream::iter(selected)
            .map(|(idx, task, deps)| async move {
                (idx, run_one_task(mcp, project_id, &task, activity, &deps).await)
            })
            .buffer_unordered(MAX_PARALLEL_TASKS)
            .collect()
            .await;
        results.sort_by_key(|(idx, _)| *idx);

        // B1: process ALL results before stopping. Every Done sibling MUST reach `review` even
        // if another task in the same batch blocked — otherwise the Done task is silently left
        // `wip` (work lost + re-delegated next run, and its dependents stall). Record the FIRST
        // block (idx order) and return it AFTER the loop. W5: count the set_reviews applied this
        // batch (`newly_reviewed`) — the in-memory `plan` is not mutated, so the report's
        // `completed` must add them to the start-of-iteration count.
        let mut first_block: Option<BlockedTask> = None;
        let mut newly_reviewed = 0usize;
        for (idx, verdict) in results {
            let task = &plan[idx];
            match verdict {
                MiniVerdict::Done(evidence) => {
                    if let Err(e) = set_review(mcp, project_id, &task.id, &evidence).await {
                        let reason = format!(
                            "mini finished but could not set the task to review: {}; \
                             please update it manually",
                            elide(&e)
                        );
                        activity.milestone(
                            &format!("review-update failed {}", task.id),
                            Node::Terra,
                        );
                        if first_block.is_none() {
                            first_block = Some(BlockedTask {
                                id: task.id.clone(),
                                title: task.title.clone(),
                                status: STATUS_WIP.to_string(),
                                reason,
                            });
                        }
                        continue;
                    }
                    newly_reviewed += 1;
                    activity.milestone(&format!("review {}", task.id), Node::Sage);
                    // v6 Phase 4: record this task's summary so its dependents can be
                    // spoon-fed what it produced (consumed in build_spawn_params).
                    summaries.insert(task.id.clone(), evidence);
                }
                MiniVerdict::Blocked(reason) => {
                    let (status, reason) =
                        block_best_effort(mcp, project_id, activity, &task.id, reason).await;
                    if status == STATUS_BLOCKED {
                        activity.milestone(&format!("blocked {}", task.id), Node::Terra);
                    }
                    if first_block.is_none() {
                        first_block = Some(BlockedTask {
                            id: task.id.clone(),
                            title: task.title.clone(),
                            status,
                            reason,
                        });
                    }
                }
            }
        }
        if let Some(blocked) = first_block {
            return Ok(RunReport {
                completed: completed + newly_reviewed,
                total,
                blocked: Some(blocked),
            });
        }
    }

    Err("runner exceeded its iteration bound (internal invariant violated)".to_string())
}

/// The set of FINISHED (review/done) task ids — the dependency-satisfaction basis.
fn finished_ids(plan: &[TaskView]) -> HashSet<&str> {
    plan.iter()
        .filter(|t| t.status == STATUS_REVIEW || t.status == STATUS_DONE)
        .map(|t| t.id.as_str())
        .collect()
}

/// W3: the ONE dependency rule, shared by `next_runnable` (one ready) and `ready_batch` (all
/// ready) so they can never diverge. A task is READY when its status is `todo`/`wip` AND every
/// `depends_on` id resolves to a finished (review/done) task. A dep id absent from the plan is
/// UNSATISFIED (cannot be confirmed finished), so the task waits.
fn task_is_ready(t: &TaskView, finished: &HashSet<&str>) -> bool {
    (t.status == STATUS_TODO || t.status == STATUS_WIP)
        && t.depends_on.iter().all(|d| finished.contains(d.as_str()))
}

/// All indices that are READY — the multi-result peer of `next_runnable`'s Ready case.
fn ready_batch(plan: &[TaskView]) -> Vec<usize> {
    let finished = finished_ids(plan);
    plan.iter()
        .enumerate()
        .filter(|(_, t)| task_is_ready(t, &finished))
        .map(|(i, _)| i)
        .collect()
}

async fn run_one_task(
    mcp: &dyn McpBackend,
    project_id: &str,
    task: &TaskView,
    activity: &Activity,
    dep_summaries: &[(String, String)],
) -> MiniVerdict {
    if let Err(e) = claim_task(mcp, project_id, &task.id).await {
        return MiniVerdict::Blocked(format!("could not claim the task: {}", elide(&e)));
    }
    activity.milestone(
        &format!("running {}: {}", task.id, elide(&task.title)),
        Node::Hollow,
    );
    let params = build_spawn_params(task, dep_summaries);
    // ROLE UNTANGLE Phase 4: a "main"-weight task goes to the Main coder (the
    // always-agentic sandboxed engine); everything else keeps the mini path.
    let tool = if task.weight.trim() == "main" { "spawn_main_coder" } else { "spawn_mini_coder" };
    match mcp.call_tool(tool, params).await {
        Ok(text) => parse_mini_status(&text),
        Err(e) => MiniVerdict::Blocked(format!("{tool} failed: {}", elide(&e))),
    }
}

/// Read the board via `project_get` and build the VIEW list of all plan-tagged tasks
/// (those carrying a non-empty `planId`). Manual tasks (no planId) are EXCLUDED — the
/// runner only auto-executes plan tasks (decision c). A transport error or an
/// unparseable/`state.tasks`-less response is a clear `Err` the caller surfaces.
async fn read_plan_views(mcp: &dyn McpBackend, project_id: &str) -> Result<Vec<TaskView>, String> {
    let text = mcp
        .call_tool(
            "project_get",
            serde_json::json!({ "project_id": project_id }),
        )
        .await
        .map_err(|e| format!("project_get failed: {e}"))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("project_get returned unparseable JSON: {e}"))?;
    let tasks = value
        .get("state")
        .and_then(|s| s.get("tasks"))
        .and_then(|t| t.as_array())
        .ok_or_else(|| "project_get result has no `state.tasks` array".to_string())?;

    let mut views = Vec::new();
    for t in tasks {
        // Only PLAN tasks (a non-empty planId) are in scope. An absent/null/empty planId
        // is a manual task — left untouched by the runner.
        let plan_id = t
            .get("planId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if plan_id.is_empty() {
            continue;
        }
        let id = t
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if id.is_empty() {
            // A task with no id is unusable for the DAG; skip it rather than abort the
            // whole run on one malformed board entry.
            continue;
        }
        let status = t
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        // Optional fields may be ABSENT when empty (the 1a server omits empty scope/
        // acceptance/dependsOn); default to empty.
        let depends_on = string_array(t.get("dependsOn"));
        let scope = string_array(t.get("scope"));
        let acceptance = t
            .get("acceptance")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let evidence = t
            .get("evidence")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let updated_at = t
            .get("updatedAt")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let weight = t
            .get("weight")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        views.push(TaskView {
            id,
            title: t
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            status,
            depends_on,
            scope,
            acceptance,
            evidence,
            plan_id,
            updated_at,
            weight,
        });
    }
    Ok(views)
}

/// Extract a `Vec<String>` from a JSON value that may be absent / null / a string array.
/// Non-string entries are dropped (defensive against a malformed board entry).
fn string_array(v: Option<&serde_json::Value>) -> Vec<String> {
    v.and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether there are ANY plan-tagged tasks in the view list (regardless of their status).
/// Used by `run_tasks` to distinguish "no plan exists" from "plan is finished".
fn has_plan_tasks(views: &[TaskView]) -> bool {
    !views.is_empty()
}

/// Pick the ACTIVE plan id among all plan-tagged tasks: group by `planId`, keep groups
/// with ≥1 task NOT in {review, done} (an unfinished plan), and:
/// * 0 unfinished groups → `None` (nothing to run — every plan is finished, or there are
///   no plan tasks at all; the caller MUST check `has_plan_tasks` to distinguish them).
/// * exactly 1 → that planId.
/// * >1 → the group whose tasks have the most-recent `updatedAt` (lexicographic max of the
///   > ISO-8601 timestamps within each group; the freshest-touched plan wins). The
///   > ambiguous-multi-plan case is an edge we document + resolve deterministically; the
///   > caller logs which planId was chosen.
fn select_active_plan(views: &[TaskView]) -> Option<String> {
    // plan_id -> (has_unfinished, max_updated_at)
    let mut groups: HashMap<&str, (bool, &str)> = HashMap::new();
    for t in views {
        let unfinished = t.status != STATUS_REVIEW && t.status != STATUS_DONE;
        let entry = groups.entry(t.plan_id.as_str()).or_insert((false, ""));
        entry.0 = entry.0 || unfinished;
        if t.updated_at.as_str() > entry.1 {
            entry.1 = t.updated_at.as_str();
        }
    }
    // Among the UNFINISHED groups, pick the one with the most-recent updatedAt. Tie-break
    // on the planId string so the choice is fully deterministic (stable across reads).
    groups
        .into_iter()
        .filter(|(_, (unfinished, _))| *unfinished)
        .max_by(|(a_id, (_, a_ts)), (b_id, (_, b_ts))| a_ts.cmp(b_ts).then_with(|| a_id.cmp(b_id)))
        .map(|(id, _)| id.to_string())
}

/// Find the next thing to do in the active plan. A task is READY when its status is `todo`
/// or `wip` (an interrupted prior run / concurrent move, re-attemptable) AND every
/// `depends_on` id resolves to a task in {review, done} (decision b "tira dritto": a
/// finished-but-not-yet-verified predecessor SATISFIES a dependency). A `blocked` task is
/// never ready. A dep id not present in the plan is treated as UNSATISFIED (it cannot be
/// confirmed finished), so the task waits rather than runs prematurely.
fn next_runnable(plan: &[TaskView]) -> Frontier {
    let finished = finished_ids(plan);

    let mut non_finished_exists = false;
    let mut first_blocked: Option<usize> = None;
    let mut first_non_ready: Option<usize> = None;
    for (i, t) in plan.iter().enumerate() {
        if t.status == STATUS_REVIEW || t.status == STATUS_DONE {
            continue;
        }
        non_finished_exists = true;
        if task_is_ready(t, &finished) {
            return Frontier::Ready(i);
        }
        if t.status == STATUS_BLOCKED && first_blocked.is_none() {
            first_blocked = Some(i);
        }
        if first_non_ready.is_none() {
            first_non_ready = Some(i);
        }
    }

    if !non_finished_exists {
        Frontier::AllDone
    } else {
        // Some non-finished task exists but none is ready: a blocked task and/or tasks
        // waiting on a non-finished predecessor. Name the BLOCKED root cause when one
        // exists (the actionable thing for the human), regardless of its index; otherwise
        // the first waiter. `first_non_ready` is necessarily Some here.
        Frontier::Stalled(
            first_blocked
                .or(first_non_ready)
                .expect("invariant: non_finished_exists implies first_non_ready.is_some()"),
        )
    }
}

/// Count active-plan tasks currently FINISHED (`review` or `done`).
fn count_finished(plan: &[TaskView]) -> usize {
    plan.iter()
        .filter(|t| t.status == STATUS_REVIEW || t.status == STATUS_DONE)
        .count()
}

/// Claim a task to establish the lease + auto-advance `todo`→`wip`. Params:
/// `{project_id, task_id}` (the backend injects role/agent_id/session_token). A non-empty
/// text result is success; a transport / tool error is surfaced as `Err`.
async fn claim_task(mcp: &dyn McpBackend, project_id: &str, task_id: &str) -> Result<(), String> {
    mcp.call_tool(
        "project_claim_task",
        serde_json::json!({ "project_id": project_id, "task_id": task_id }),
    )
    .await
    .map(|_| ())
}

/// Set a task to `review` on the board. Params:
/// `{project_id, task_id, status: "review", evidence}` (the server REQUIRES ≥12-char
/// evidence for the review transition; the backend injects the identity).
async fn set_review(mcp: &dyn McpBackend, project_id: &str, task_id: &str, evidence: &str) -> Result<(), String> {
    mcp.call_tool(
        "project_update_status",
        serde_json::json!({
            "project_id": project_id,
            "task_id": task_id,
            "status": STATUS_REVIEW,
            "evidence": evidence,
        }),
    )
    .await
    .map(|_| ())
    .map_err(|e| format!("could not set {task_id} to review: {e}"))
}

/// Set a task to `blocked` on the board, carrying the block reason as evidence. Params:
/// `{project_id, task_id, status: "blocked", evidence}`. The reason is folded into a
/// ≥12-char evidence string (the server's floor for the blocked transition) and capped so
/// a verbose mini error cannot overflow the field.
async fn set_blocked(
    mcp: &dyn McpBackend,
    project_id: &str,
    task_id: &str,
    reason: &str,
) -> Result<(), String> {
    // Always ≥12 chars: the fixed prefix alone clears the floor even for an empty reason.
    let evidence = cap_chars(
        &format!("devboule runner blocked this task: {reason}"),
        1_200,
    );
    mcp.call_tool(
        "project_update_status",
        serde_json::json!({
            "project_id": project_id,
            "task_id": task_id,
            "status": STATUS_BLOCKED,
            "evidence": evidence,
        }),
    )
    .await
    .map(|_| ())
    .map_err(|e| format!("could not set {task_id} to blocked: {e}"))
}

/// Best-effort block of a task that is ALREADY stopping the run, returning the truthful
/// `(board_status, reason)` to put in the `RunReport`.
///
/// W2: the previous `let _ = set_blocked(...)` swallowed a failed block-transition, so the
/// run would report the task as `blocked` while the BOARD silently stayed `wip` — a
/// discrepancy invisible to the human. We keep the call best-effort (a failed block must
/// NOT escalate to a transport `Err` and abort the whole run — the originating block
/// reason is the important signal), but we make the failure OBSERVABLE:
///   * on success → `(STATUS_BLOCKED, reason)`: the board moved, the report matches it;
///   * on failure → emit a distinct `block-write failed <id>` Console milestone AND fold
///     the write error into the reason, and report the board's REAL status (`wip` — the
///     card never left the claimed state) so the report does not falsely claim `blocked`.
async fn block_best_effort(
    mcp: &dyn McpBackend,
    project_id: &str,
    activity: &Activity,
    task_id: &str,
    reason: String,
) -> (String, String) {
    match set_blocked(mcp, project_id, task_id, &reason).await {
        Ok(()) => (STATUS_BLOCKED.to_string(), reason),
        Err(e) => {
            activity.milestone(&format!("block-write failed {task_id}"), Node::Terra);
            (
                STATUS_WIP.to_string(),
                format!("{reason} (NOTE: could not update the board to blocked: {})", elide(&e)),
            )
        }
    }
}

/// Build the `spawn_mini_coder` argument object for one task: the task text is the title +
/// the deterministic acceptance check (so the mini knows the bar); `files` is the task's
/// MODIFY scope (the write allowlist); `write: true` selects the edit arm. The whole
/// composed task is char-capped so it can never overflow the directive.
///
/// DELIBERATE: `allow_oracle: true` is granted on EVERY runner-delegated task — an
/// asymmetry with the orchestrator's own `SpawnMini` dispatch (which omits it). A planned
/// task carries only ≤3 MODIFY files, so the implementing mini needs read-only codebase
/// grounding to do the work correctly; `oracle_*` is read-only + project-confined, so this
/// widens READS only, never the WRITE allowlist (still exactly `files`).
///
/// W3 (TRUST MODEL of the folded fields): `title` and `acceptance` are MODEL-GENERATED —
/// the planner wrote them and they were stored on the Kanban card (already stripped of
/// invisible/BiDi control chars by the `project_create_plan_tasks` MCP tool before they
/// could persist). They are the SAME trust tier as the directive task itself (supervisor
/// prose), NOT a higher-privileged channel: nothing here can widen the mini's write
/// allowlist (still exactly `task.scope`) or its tool grants. The `title` IS the task (the
/// primary instruction, placed first); the `acceptance` is appended behind a clearly
/// LABELLED `Acceptance:` boundary so the mini reads it as the bar to meet, not as a fresh
/// instruction stream. The mini's own prompt-injection firewall (it treats the whole
/// delegated task as untrusted input, see `mini_coder.rs`) is the BACKSTOP; this is a
/// labelling/bounding layer, not the security boundary itself.
/// Two task scopes OVERLAP when they name at least one file in common — such tasks must
/// not run in the same parallel batch (both minis would write the shared file in one
/// working tree). Scopes are tiny (≤ MAX_TASK_SCOPE), so a linear check is fine.
fn scopes_overlap(a: &[String], b: &[String]) -> bool {
    a.iter().any(|p| b.contains(p))
}

fn build_spawn_params(task: &TaskView, dep_summaries: &[(String, String)]) -> serde_json::Value {
    let mut text = task.title.trim().to_string();
    if !task.acceptance.trim().is_empty() {
        // Bound the model-generated acceptance behind its own LABEL (see the trust-model
        // note above) so it reads as the success bar, distinct from the title/task above.
        text.push_str("\n\nAcceptance: ");
        text.push_str(task.acceptance.trim());
    }
    // v6 Phase 4 (SUMMARY-feeding): front-load what this task's completed dependencies
    // produced, so the mini has the context without re-deriving it. Bounded per-dependency;
    // the whole text is capped below at MAX_DELEGATED_TASK_CHARS.
    if !dep_summaries.is_empty() {
        // Cap the NUMBER of dep summaries so a many-dependency "integration" task can't
        // blow the per-spawn char budget and silently drop the tail (the whole text is
        // capped below, which truncates from the END). Keep the first N, name the rest.
        const MAX_DEP_SUMMARIES: usize = 8;
        text.push_str("\n\nContext from completed dependencies (already done):");
        for (id, summary) in dep_summaries.iter().take(MAX_DEP_SUMMARIES) {
            text.push_str(&format!("\n- {}: {}", id, cap_chars(summary.trim(), 400)));
        }
        let omitted = dep_summaries.len().saturating_sub(MAX_DEP_SUMMARIES);
        if omitted > 0 {
            text.push_str(&format!(
                "\n- (+{omitted} earlier dependencies omitted for length)"
            ));
        }
    }
    let text = cap_chars(&text, MAX_DELEGATED_TASK_CHARS);
    serde_json::json!({
        "task": text,
        "files": task.scope,
        "write": true,
        "allow_oracle": true,
    })
}

/// Reduce a `spawn_mini_coder` result text to a [`MiniVerdict`]. The tool returns
/// `{"directiveId":..,"result":{"status":<s>,"error"?:<e>}}`. Only `status == "done"` is
/// success; ANY other status — or an unparseable / status-less body — is a block
/// (conservative: never treat a non-success as a go-ahead).
fn parse_mini_status(text: &str) -> MiniVerdict {
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return MiniVerdict::Blocked("mini returned an unparseable result".to_string()),
    };
    let result = value.get("result");
    let status = result
        .and_then(|r| r.get("status"))
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if status == "done" {
        // Build review evidence from the mini's real output and files_touched.
        let output = result
            .and_then(|r| r.get("output"))
            .and_then(|o| o.as_str())
            .unwrap_or("")
            .trim();
        let files_touched = result
            .and_then(|r| r.get("files_touched"))
            .and_then(|f| f.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(|s| s.to_string()))
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default();
        let evidence = format!(
            "mini reports done; output: {}{}",
            output,
            if files_touched.is_empty() {
                String::new()
            } else {
                format!(
                    "; touched: {}",
                    files_touched
                        .iter()
                        .map(|f| f.as_str())
                        .collect::<Vec<&str>>()
                        .join(", ")
                )
            }
        );
        // Cap the evidence at 1_200 chars (same ceiling as `set_blocked`); a verbose
        // mini output cannot bloat the board field. The fixed prefix already guarantees
        // ≥12 chars, so the minimum is satisfied.
        let evidence = cap_chars(&evidence, 1_200);
        return MiniVerdict::Done(evidence);
    }
    // For needs_clarification, use the `question` field as the reason (it holds the
    // mini's actual clarification question); fall back to `error` only if `question` is
    // empty/None.
    let detail = result
        .and_then(|r| r.get("question"))
        .and_then(|q| q.as_str())
        .unwrap_or("")
        .trim();
    let reason = if status.is_empty() {
        "mini returned no status".to_string()
    } else if detail.is_empty() {
        // Fall back to `error` when `question` is empty (e.g. a clean needs_clarification)
        let error = result
            .and_then(|r| r.get("error"))
            .and_then(|e| e.as_str())
            .unwrap_or("")
            .trim();
        if error.is_empty() {
            format!("mini status '{status}'")
        } else {
            format!("mini status '{status}': {}", elide(error))
        }
    } else {
        format!("mini status '{status}': {}", elide(detail))
    };
    MiniVerdict::Blocked(reason)
}

/// Truncate to at most `cap` CHARS (never splitting a codepoint), with a marker when cut.
fn cap_chars(s: &str, cap: usize) -> String {
    if s.chars().count() <= cap {
        return s.to_string();
    }
    const MARKER: &str = "\n[…truncated]";
    let marker_len = MARKER.chars().count();
    if cap <= marker_len {
        return MARKER.chars().take(cap).collect();
    }
    let kept: String = s.chars().take(cap - marker_len).collect();
    format!("{kept}{MARKER}")
}

/// One-line elision for milestone labels / reasons (≤ 80 chars, newlines flattened).
fn elide(s: &str) -> String {
    const MAX: usize = 80;
    let one_line = s.replace(['\n', '\r'], " ");
    let trimmed = one_line.trim();
    if trimmed.chars().count() <= MAX {
        return trimmed.to_string();
    }
    let cut: String = trimmed.chars().take(MAX).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;

    #[test]
    fn scopes_overlap_detects_shared_file() {
        assert!(
            scopes_overlap(&["a.rs".into(), "b.rs".into()], &["b.rs".into()]),
            "shared b.rs must overlap"
        );
        assert!(!scopes_overlap(&["a.rs".into()], &["b.rs".into()]), "disjoint must not overlap");
        let empty: &[String] = &[];
        assert!(!scopes_overlap(empty, &["b.rs".into()]), "empty scope never overlaps");
    }

    #[test]
    fn build_spawn_params_front_loads_dependency_summaries() {
        let task = TaskView {
            id: "T2".into(),
            title: "implement login".into(),
            status: "todo".into(),
            depends_on: vec!["T1".into()],
            scope: vec!["src/auth.rs".into()],
            acceptance: "cargo test auth".into(),
            evidence: String::new(),
            plan_id: "P".into(),
            updated_at: "t".into(),
            weight: String::new(),
        };
        // With a completed-dependency summary → it is front-loaded.
        let deps = vec![("T1".to_string(), "created the auth module in auth.rs".to_string())];
        let params = build_spawn_params(&task, &deps);
        let text = params["task"].as_str().unwrap();
        assert!(text.contains("Context from completed dependencies"), "dep section present");
        assert!(text.contains("T1"), "dep id present");
        assert!(text.contains("created the auth module"), "dep summary present");
        // With no dependency summaries → no section (unchanged behavior).
        let none = build_spawn_params(&task, &[]);
        assert!(
            !none["task"].as_str().unwrap().contains("Context from completed dependencies"),
            "no dep section when there are no summaries"
        );
    }

    /// A MOCK Kanban backend. It holds the project's task list (mutated in place as the
    /// runner advances cards, so a re-read reflects prior transitions), returns SCRIPTED
    /// `spawn_mini_coder` verdicts per delegation, and RECORDS every
    /// `project_update_status` / `project_claim_task` / `spawn_mini_coder` call so tests
    /// can assert the board transitions + the delegated payload.
    struct KanbanMock {
        tasks: Mutex<Vec<serde_json::Value>>,
        mini_results: Vec<String>,
        mini_cursor: Mutex<usize>,
        calls: Mutex<Vec<(String, serde_json::Value)>>,
    }

    impl KanbanMock {
        /// `tasks` is the initial board (each a JSON task object). `mini_statuses` is the
        /// ordered `result.status` per `spawn_mini_coder` delegation (a call past the
        /// script returns `done`, so an over-running test still terminates via the
        /// done-detection / iteration bound, not an exhausted script).
        fn new(tasks: Vec<serde_json::Value>, mini_statuses: &[&str]) -> Self {
            let mini_results = mini_statuses
                .iter()
                .map(|s| {
                    serde_json::json!({"directiveId": "d", "result": {"status": s}}).to_string()
                })
                .collect();
            Self {
                tasks: Mutex::new(tasks),
                mini_results,
                mini_cursor: Mutex::new(0),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn call_names(&self) -> Vec<String> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .map(|(n, _)| n.clone())
                .collect()
        }

        fn count(&self, name: &str) -> usize {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(n, _)| n == name)
                .count()
        }

        /// Every `project_update_status` call's (task_id, status) pair, in order.
        fn status_updates(&self) -> Vec<(String, String)> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(n, _)| n == "project_update_status")
                .map(|(_, p)| {
                    (
                        p["task_id"].as_str().unwrap_or("").to_string(),
                        p["status"].as_str().unwrap_or("").to_string(),
                    )
                })
                .collect()
        }

        /// The `files` of each `spawn_mini_coder` delegation, in order.
        fn delegated_files(&self) -> Vec<serde_json::Value> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(n, _)| n == "spawn_mini_coder")
                .map(|(_, p)| p["files"].clone())
                .collect()
        }

        /// The current status of a task id (from the mutated board).
        fn status_of(&self, id: &str) -> String {
            self.tasks
                .lock()
                .unwrap()
                .iter()
                .find(|t| t["id"].as_str() == Some(id))
                .and_then(|t| t["status"].as_str().map(str::to_string))
                .unwrap_or_default()
        }

        fn project_get_body(&self) -> String {
            serde_json::json!({
                "metadata": {"id": "proj"},
                "state": {"tasks": self.tasks.lock().unwrap().clone()},
            })
            .to_string()
        }

        fn set_status(&self, id: &str, status: &str) {
            let mut tasks = self.tasks.lock().unwrap();
            if let Some(t) = tasks.iter_mut().find(|t| t["id"].as_str() == Some(id)) {
                t["status"] = serde_json::json!(status);
            }
        }
    }

    #[async_trait]
    impl McpBackend for KanbanMock {
        async fn call_tool(&self, name: &str, params: serde_json::Value) -> Result<String, String> {
            self.calls
                .lock()
                .unwrap()
                .push((name.to_string(), params.clone()));
            match name {
                "project_get" => Ok(self.project_get_body()),
                "project_claim_task" => {
                    // Mirror the server: a `todo` task auto-advances to `wip`.
                    let id = params["task_id"].as_str().unwrap_or("");
                    if self.status_of(id) == STATUS_TODO {
                        self.set_status(id, STATUS_WIP);
                    }
                    Ok(serde_json::json!({"claimed": id}).to_string())
                }
                "project_update_status" => {
                    let id = params["task_id"].as_str().unwrap_or("").to_string();
                    let status = params["status"].as_str().unwrap_or("").to_string();
                    // The server requires ≥12-char evidence for review/blocked; assert the
                    // runner always supplies it (contract guard).
                    if status == STATUS_REVIEW || status == STATUS_BLOCKED {
                        let ev = params["evidence"].as_str().unwrap_or("");
                        assert!(
                            ev.chars().count() >= 12,
                            "review/blocked needs >=12-char evidence, got {ev:?}"
                        );
                    }
                    self.set_status(&id, &status);
                    Ok(serde_json::json!({"metadata": {"id": "proj"}}).to_string())
                }
                "spawn_mini_coder" | "spawn_main_coder" => {
                    let mut cursor = self.mini_cursor.lock().unwrap();
                    let idx = *cursor;
                    *cursor += 1;
                    Ok(self.mini_results.get(idx).cloned().unwrap_or_else(|| {
                        serde_json::json!({"directiveId": "d", "result": {"status": "done"}})
                            .to_string()
                    }))
                }
                other => Err(format!("unexpected tool {other}")),
            }
        }
    }

    /// An MCP backend whose `project_get` is fine but every `spawn_mini_coder` errors — to
    /// prove a transport error blocks the task (never panics).
    struct MiniErrMock {
        tasks: Vec<serde_json::Value>,
    }
    #[async_trait]
    impl McpBackend for MiniErrMock {
        async fn call_tool(
            &self,
            name: &str,
            _params: serde_json::Value,
        ) -> Result<String, String> {
            match name {
                "project_get" => Ok(serde_json::json!({
                    "metadata": {"id": "proj"},
                    "state": {"tasks": self.tasks.clone()},
                })
                .to_string()),
                "project_claim_task" => Ok("{}".to_string()),
                "project_update_status" => {
                    Ok(serde_json::json!({"metadata": {"id": "proj"}}).to_string())
                }
                "spawn_mini_coder" | "spawn_main_coder" => Err("backend offline".to_string()),
                other => Err(format!("unexpected tool {other}")),
            }
        }
    }

    /// W2: a backend whose board reads + claim + delegation work, the mini ESCALATES
    /// (a block verdict), but the `project_update_status` → `blocked` write FAILS. Proves
    /// `block_best_effort` keeps the run from erroring, reports the board's REAL status
    /// (`wip`, not a false `blocked`), and folds the write error into the reason.
    struct BlockWriteFailsMock {
        tasks: Vec<serde_json::Value>,
    }
    #[async_trait]
    impl McpBackend for BlockWriteFailsMock {
        async fn call_tool(
            &self,
            name: &str,
            params: serde_json::Value,
        ) -> Result<String, String> {
            match name {
                "project_get" => Ok(serde_json::json!({
                    "metadata": {"id": "proj"},
                    "state": {"tasks": self.tasks.clone()},
                })
                .to_string()),
                "project_claim_task" => Ok("{}".to_string()),
                "project_update_status" => {
                    // The ONLY failing transition is the runner's block write.
                    if params["status"].as_str() == Some(STATUS_BLOCKED) {
                        return Err("board write rejected (stale revision)".to_string());
                    }
                    Ok(serde_json::json!({"metadata": {"id": "proj"}}).to_string())
                }
                // Escalate -> a block verdict that drives the runner into set_blocked.
                "spawn_mini_coder" | "spawn_main_coder" => Ok(serde_json::json!(
                    {"directiveId": "d", "result": {"status": "escalated"}}
                )
                .to_string()),
                other => Err(format!("unexpected tool {other}")),
            }
        }
    }

    /// Build a plan-tagged Kanban task JSON object.
    fn ktask(
        id: &str,
        plan: &str,
        deps: &[&str],
        status: &str,
        updated_at: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "title": format!("title {id}"),
            "status": status,
            "planId": plan,
            "scope": [format!("src/{id}.rs")],
            "acceptance": "cargo test",
            "dependsOn": deps,
            "updatedAt": updated_at,
        })
    }

    /// Build a plan-tagged Kanban task JSON object with a `weight` field.
    fn ktask_with_weight(
        id: &str,
        plan: &str,
        deps: &[&str],
        status: &str,
        updated_at: &str,
        weight: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "title": format!("title {id}"),
            "status": status,
            "planId": plan,
            "scope": [format!("src/{id}.rs")],
            "acceptance": "cargo test",
            "dependsOn": deps,
            "updatedAt": updated_at,
            "weight": weight,
        })
    }

    /// A MANUAL task (no planId) — the runner must ignore it entirely.
    fn manual_task(id: &str, status: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "title": format!("manual {id}"),
            "status": status,
            "updatedAt": "2026-01-01T00:00:00Z",
        })
    }

    fn disabled() -> Activity {
        Activity::disabled()
    }

    /// ROLE UNTANGLE Phase 4: a task with `weight: "main"` must delegate to
    /// `spawn_main_coder` (not `spawn_mini_coder`), while a task without weight
    /// keeps the mini path.
    #[tokio::test]
    async fn main_weight_task_delegates_to_spawn_main_coder() {
        // A plan with one ready task carrying `weight: "main"` — the runner must
        // call `spawn_main_coder`, NOT `spawn_mini_coder`.
        let mock = KanbanMock::new(
            vec![ktask_with_weight(
                "T001",
                "p1",
                &[],
                STATUS_TODO,
                "2026-06-17T10:00:00Z",
                "main",
            )],
            &["done"],
        );
        let report = run_tasks(&mock, "proj", &disabled()).await.unwrap();
        assert_eq!(report.blocked, None);
        assert_eq!((report.completed, report.total), (1, 1));
        assert_eq!(
            mock.count("spawn_main_coder"),
            1,
            "a main-weight task uses spawn_main_coder"
        );
        assert_eq!(
            mock.count("spawn_mini_coder"),
            0,
            "no spawn_mini_coder for a main-weight task"
        );
    }

    /// A task without a weight (or with an empty weight) must keep using the
    /// spawn_mini_coder path — byte-identical to the existing behavior.
    #[tokio::test]
    async fn absent_weight_task_keeps_spawn_mini_coder() {
        // A plan with one ready task with NO weight — must delegate to
        // `spawn_mini_coder`, as before.
        let mock = KanbanMock::new(
            vec![ktask(
                "T001",
                "p1",
                &[],
                STATUS_TODO,
                "2026-06-17T10:00:00Z",
            )],
            &["done"],
        );
        let report = run_tasks(&mock, "proj", &disabled()).await.unwrap();
        assert_eq!(report.blocked, None);
        assert_eq!((report.completed, report.total), (1, 1));
        assert_eq!(
            mock.count("spawn_mini_coder"),
            1,
            "absent weight keeps spawn_mini_coder"
        );
        assert_eq!(
            mock.count("spawn_main_coder"),
            0,
            "no spawn_main_coder for absent weight"
        );
    }

    // --- Active-plan detection (0 / 1 / multiple) ----------------------------

    #[tokio::test]
    async fn no_plan_tasks_is_an_error_not_a_silent_no_op() {
        // Only manual tasks on the board → no plan-tagged tasks at all → Err (B1).
        // Returning Ok("Ran 0/0; all finished") here would mislead the orchestrator into
        // thinking the plan ran when `plan` was never called.
        let mock = KanbanMock::new(
            vec![manual_task("T001", "todo"), manual_task("T002", "wip")],
            &[],
        );
        let err = run_tasks(&mock, "proj", &disabled())
            .await
            .expect_err("no plan-tagged tasks must be an Err");
        assert!(
            err.contains("no active plan"),
            "error names the missing plan: {err}"
        );
        assert!(
            err.contains("plan` first"),
            "error tells user to run plan: {err}"
        );
        assert_eq!(
            mock.count("spawn_mini_coder"),
            0,
            "nothing delegated with no plan"
        );
        assert_eq!(mock.count("project_update_status"), 0, "no card touched");
    }

    #[tokio::test]
    async fn a_fully_finished_plan_reports_real_counts_not_zero() {
        // Every plan task already in review/done → select_active_plan returns None (no
        // unfinished group) but plan-tagged tasks DO exist → B1 case (b): idempotent
        // no-op that reports the REAL finished count, NOT 0/0.
        let mock = KanbanMock::new(
            vec![
                ktask("T001", "p1", &[], STATUS_REVIEW, "2026-06-17T10:00:00Z"),
                ktask("T002", "p1", &["T001"], STATUS_DONE, "2026-06-17T10:01:00Z"),
            ],
            &[],
        );
        let report = run_tasks(&mock, "proj", &disabled()).await.unwrap();
        // Both tasks are finished → completed=2, total=2, no block, no delegation.
        assert_eq!((report.completed, report.total), (2, 2));
        assert_eq!(report.blocked, None);
        assert_eq!(mock.count("spawn_mini_coder"), 0);
    }

    #[tokio::test]
    async fn exactly_one_active_plan_is_selected_and_run() {
        let mock = KanbanMock::new(
            vec![
                ktask("T001", "p1", &[], STATUS_TODO, "2026-06-17T10:00:00Z"),
                ktask("T002", "p1", &["T001"], STATUS_TODO, "2026-06-17T10:00:01Z"),
            ],
            &["done", "done"],
        );
        let report = run_tasks(&mock, "proj", &disabled()).await.unwrap();
        assert_eq!(report.blocked, None);
        assert_eq!((report.completed, report.total), (2, 2));
        assert_eq!(
            mock.count("spawn_mini_coder"),
            2,
            "each plan task delegated once"
        );
    }

    #[tokio::test]
    async fn multiple_active_plans_pick_the_most_recently_updated() {
        // Two active plans on one board. p2's tasks have the later updatedAt, so the runner
        // selects p2 and runs ONLY p2's task (p1 is left untouched). Manual tasks ignored.
        let mock = KanbanMock::new(
            vec![
                ktask("T001", "p1", &[], STATUS_TODO, "2026-06-17T09:00:00Z"),
                ktask("T002", "p2", &[], STATUS_TODO, "2026-06-17T12:00:00Z"),
                manual_task("T003", "todo"),
            ],
            &["done"],
        );
        let report = run_tasks(&mock, "proj", &disabled()).await.unwrap();
        assert_eq!(report.blocked, None);
        assert_eq!(
            (report.completed, report.total),
            (1, 1),
            "only p2 is the active plan"
        );
        // p2's task was driven to review; p1's task is untouched (still todo).
        assert_eq!(mock.status_of("T002"), STATUS_REVIEW);
        assert_eq!(
            mock.status_of("T001"),
            STATUS_TODO,
            "the other plan is untouched"
        );
        assert_eq!(mock.count("spawn_mini_coder"), 1);
    }

    // --- The DAG (the core) --------------------------------------------------

    #[tokio::test]
    async fn linear_plan_all_done_lands_every_task_in_review() {
        // T001 -> T002 -> T003, every mini returns done. Each finished task lands in
        // REVIEW (decision a: the runner never sets done).
        let mock = KanbanMock::new(
            vec![
                ktask("T001", "p1", &[], STATUS_TODO, "2026-06-17T10:00:00Z"),
                ktask("T002", "p1", &["T001"], STATUS_TODO, "2026-06-17T10:00:01Z"),
                ktask("T003", "p1", &["T002"], STATUS_TODO, "2026-06-17T10:00:02Z"),
            ],
            &["done", "done", "done"],
        );
        let report = run_tasks(&mock, "proj", &disabled()).await.unwrap();
        assert_eq!(report.blocked, None);
        assert_eq!((report.completed, report.total), (3, 3));
        assert_eq!(
            mock.count("spawn_mini_coder"),
            3,
            "each task delegated once, in order"
        );
        // Each task claimed (todo->wip) then set review.
        assert_eq!(mock.status_of("T001"), STATUS_REVIEW);
        assert_eq!(mock.status_of("T002"), STATUS_REVIEW);
        assert_eq!(mock.status_of("T003"), STATUS_REVIEW);
        // The runner NEVER issued a `done` update (verifier-only).
        assert!(
            mock.status_updates().iter().all(|(_, s)| s != STATUS_DONE),
            "runner must never set done"
        );
    }

    #[tokio::test]
    async fn delegation_passes_scope_as_files_write_arm_and_oracle() {
        let mock = KanbanMock::new(
            vec![ktask(
                "T001",
                "p1",
                &[],
                STATUS_TODO,
                "2026-06-17T10:00:00Z",
            )],
            &["done"],
        );
        run_tasks(&mock, "proj", &disabled()).await.unwrap();
        let files = mock.delegated_files();
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0],
            serde_json::json!(["src/T001.rs"]),
            "scope is the write allowlist"
        );
        let call = mock
            .calls
            .lock()
            .unwrap()
            .iter()
            .find(|(n, _)| n == "spawn_mini_coder")
            .unwrap()
            .1
            .clone();
        assert_eq!(call["write"], serde_json::json!(true), "write arm selected");
        assert_eq!(
            call["allow_oracle"],
            serde_json::json!(true),
            "mini may ground itself"
        );
        assert!(
            call["task"].as_str().unwrap().contains("Acceptance:"),
            "acceptance carried"
        );
    }

    #[tokio::test]
    async fn first_escalation_stops_and_blocks_the_task() {
        // T001 done(->review), T002 escalates -> runner sets T002 blocked + stops; T003
        // never reached.
        let mock = KanbanMock::new(
            vec![
                ktask("T001", "p1", &[], STATUS_TODO, "2026-06-17T10:00:00Z"),
                ktask("T002", "p1", &["T001"], STATUS_TODO, "2026-06-17T10:00:01Z"),
                ktask("T003", "p1", &["T002"], STATUS_TODO, "2026-06-17T10:00:02Z"),
            ],
            &["done", "escalated"],
        );
        let report = run_tasks(&mock, "proj", &disabled()).await.unwrap();
        let blocked = report.blocked.expect("a block");
        assert_eq!(blocked.id, "T002");
        assert!(
            blocked.reason.contains("escalated"),
            "reason names the mini status: {}",
            blocked.reason
        );
        assert_eq!(report.completed, 1, "only T001 finished");
        assert_eq!(mock.count("spawn_mini_coder"), 2, "T003 never delegated");
        assert_eq!(mock.status_of("T001"), STATUS_REVIEW);
        assert_eq!(mock.status_of("T002"), STATUS_BLOCKED);
        assert_eq!(mock.status_of("T003"), STATUS_TODO, "T003 untouched");
    }

    #[tokio::test]
    async fn failed_block_write_is_observable_not_swallowed() {
        // W2: the mini escalates (a block verdict) but the board's blocked write FAILS.
        // The run must NOT error; instead it returns a RunReport whose blocked entry tells
        // the truth — the board stayed `wip` (the write failed) and the reason carries the
        // write error — and a `block-write failed` milestone is emitted to the Console.
        let activity_file = std::env::temp_dir().join(format!(
            "devboule-w2-{}-{}.jsonl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&activity_file);
        let activity = Activity::with_path(&activity_file);

        let mock = BlockWriteFailsMock {
            tasks: vec![ktask("T001", "p1", &[], STATUS_TODO, "2026-06-17T10:00:00Z")],
        };
        let report = run_tasks(&mock, "proj", &activity).await.unwrap();
        let blocked = report.blocked.expect("a block report");
        assert_eq!(blocked.id, "T001");
        // The board write failed, so the report must NOT claim `blocked`: the card is
        // still `wip` (the claim drove it there and it never moved on).
        assert_eq!(
            blocked.status, STATUS_WIP,
            "a failed block write must not be reported as blocked"
        );
        assert!(
            blocked.reason.contains("escalated"),
            "the original block reason is preserved: {}",
            blocked.reason
        );
        assert!(
            blocked.reason.contains("could not update the board"),
            "the write failure must be folded into the reason: {}",
            blocked.reason
        );

        // The Console milestone for the failed write must be on disk.
        let log = std::fs::read_to_string(&activity_file).unwrap_or_default();
        assert!(
            log.contains("block-write failed T001"),
            "a block-write-failed milestone must be emitted, got: {log}"
        );
        let _ = std::fs::remove_file(&activity_file);
    }

    #[tokio::test]
    async fn resume_skips_review_and_done_tasks() {
        // T001 already in review (a prior batch), T002 done; only T003 is delegated. Proves
        // the decision-b "satisfied on review" AND that finished tasks are not re-run.
        let mock = KanbanMock::new(
            vec![
                ktask("T001", "p1", &[], STATUS_REVIEW, "2026-06-17T10:00:00Z"),
                ktask("T002", "p1", &["T001"], STATUS_DONE, "2026-06-17T10:00:01Z"),
                ktask("T003", "p1", &["T002"], STATUS_TODO, "2026-06-17T10:00:02Z"),
            ],
            &["done"],
        );
        let report = run_tasks(&mock, "proj", &disabled()).await.unwrap();
        assert_eq!(report.blocked, None);
        assert_eq!((report.completed, report.total), (3, 3));
        assert_eq!(mock.count("spawn_mini_coder"), 1, "only T003 delegated");
        assert_eq!(
            mock.delegated_files()[0],
            serde_json::json!(["src/T003.rs"])
        );
    }

    #[tokio::test]
    async fn dependency_is_satisfied_by_a_review_predecessor() {
        // Decision b "tira dritto": T002 depends on T001; T001 is in REVIEW (not done).
        // T002 must still run — a review predecessor satisfies the dependency.
        let mock = KanbanMock::new(
            vec![
                ktask("T001", "p1", &[], STATUS_REVIEW, "2026-06-17T10:00:00Z"),
                ktask("T002", "p1", &["T001"], STATUS_TODO, "2026-06-17T10:00:01Z"),
            ],
            &["done"],
        );
        let report = run_tasks(&mock, "proj", &disabled()).await.unwrap();
        assert_eq!(report.blocked, None);
        assert_eq!((report.completed, report.total), (2, 2));
        assert_eq!(
            mock.status_of("T002"),
            STATUS_REVIEW,
            "T002 ran despite T001 only in review"
        );
    }

    #[tokio::test]
    async fn diamond_dag_runs_all_respecting_dependencies() {
        // T001 -> {T002, T003} -> T004. All complete; the runner picks a valid order.
        let mock = KanbanMock::new(
            vec![
                ktask("T001", "p1", &[], STATUS_TODO, "2026-06-17T10:00:00Z"),
                ktask("T002", "p1", &["T001"], STATUS_TODO, "2026-06-17T10:00:01Z"),
                ktask("T003", "p1", &["T001"], STATUS_TODO, "2026-06-17T10:00:02Z"),
                ktask(
                    "T004",
                    "p1",
                    &["T002", "T003"],
                    STATUS_TODO,
                    "2026-06-17T10:00:03Z",
                ),
            ],
            &["done", "done", "done", "done"],
        );
        let report = run_tasks(&mock, "proj", &disabled()).await.unwrap();
        assert_eq!(report.blocked, None);
        assert_eq!((report.completed, report.total), (4, 4));
        assert!(
            ["T001", "T002", "T003", "T004"]
                .iter()
                .all(|id| mock.status_of(id) == STATUS_REVIEW),
            "every diamond task lands in review"
        );
    }

    /// A Kanban mock whose `spawn_mini_coder` verdict is keyed by TASK ID (parsed from the
    /// delegated `task` text's "title T00X"), so a CONCURRENT batch — whose call order is
    /// non-deterministic — stays deterministic per task. Board mutated in place like KanbanMock.
    struct ByIdMock {
        tasks: Mutex<Vec<serde_json::Value>>,
        statuses: std::collections::HashMap<String, String>,
        calls: Mutex<Vec<(String, serde_json::Value)>>,
    }
    impl ByIdMock {
        fn new(tasks: Vec<serde_json::Value>, statuses: &[(&str, &str)]) -> Self {
            Self {
                tasks: Mutex::new(tasks),
                statuses: statuses
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                calls: Mutex::new(Vec::new()),
            }
        }
        fn status_of(&self, id: &str) -> String {
            self.tasks
                .lock()
                .unwrap()
                .iter()
                .find(|t| t["id"].as_str() == Some(id))
                .and_then(|t| t["status"].as_str().map(str::to_string))
                .unwrap_or_default()
        }
        fn set_status(&self, id: &str, status: &str) {
            let mut tasks = self.tasks.lock().unwrap();
            if let Some(t) = tasks.iter_mut().find(|t| t["id"].as_str() == Some(id)) {
                t["status"] = serde_json::json!(status);
            }
        }
        fn count(&self, name: &str) -> usize {
            self.calls.lock().unwrap().iter().filter(|(n, _)| n == name).count()
        }
    }
    #[async_trait]
    impl McpBackend for ByIdMock {
        async fn call_tool(&self, name: &str, params: serde_json::Value) -> Result<String, String> {
            self.calls.lock().unwrap().push((name.to_string(), params.clone()));
            match name {
                "project_get" => Ok(serde_json::json!({
                    "metadata": {"id": "proj"},
                    "state": {"tasks": self.tasks.lock().unwrap().clone()},
                })
                .to_string()),
                "project_claim_task" => {
                    let id = params["task_id"].as_str().unwrap_or("");
                    if self.status_of(id) == STATUS_TODO {
                        self.set_status(id, STATUS_WIP);
                    }
                    Ok("{}".to_string())
                }
                "project_update_status" => {
                    let id = params["task_id"].as_str().unwrap_or("").to_string();
                    let status = params["status"].as_str().unwrap_or("").to_string();
                    self.set_status(&id, &status);
                    Ok(serde_json::json!({"metadata": {"id": "proj"}}).to_string())
                }
                "spawn_mini_coder" | "spawn_main_coder" => {
                    // The delegated `task` text leads with the title "title T00X" — match it to
                    // the per-id scripted status (unmatched => done, so an over-run still ends).
                    let text = params["task"].as_str().unwrap_or("");
                    let status = self
                        .statuses
                        .iter()
                        .find(|(id, _)| text.contains(id.as_str()))
                        .map(|(_, s)| s.clone())
                        .unwrap_or_else(|| "done".to_string());
                    Ok(serde_json::json!({"directiveId": "d", "result": {"status": status}})
                        .to_string())
                }
                other => Err(format!("unexpected tool {other}")),
            }
        }
    }

    #[tokio::test]
    async fn a_blocked_batch_mate_does_not_skip_a_done_siblings_review() {
        // B1 regression: two INDEPENDENT ready tasks run in ONE concurrent batch. T001 (lower
        // board idx) BLOCKS, T002 (higher idx) is DONE. The Done sibling MUST still reach
        // review — the pre-fix code processed results in idx order, hit T001's block first, and
        // returned WITHOUT applying T002's set_review (silent work loss + re-delegation).
        let mock = ByIdMock::new(
            vec![
                ktask("T001", "p1", &[], STATUS_TODO, "2026-06-17T10:00:00Z"),
                ktask("T002", "p1", &[], STATUS_TODO, "2026-06-17T10:00:01Z"),
            ],
            &[("T001", "escalated"), ("T002", "done")],
        );
        let report = run_tasks(&mock, "proj", &disabled()).await.unwrap();
        let blocked = report.blocked.expect("T001 blocks");
        assert_eq!(blocked.id, "T001");
        assert_eq!(mock.status_of("T001"), STATUS_BLOCKED);
        assert_eq!(
            mock.status_of("T002"),
            STATUS_REVIEW,
            "the Done batch-mate must reach review even though T001 blocked"
        );
        assert_eq!(report.completed, 1, "T002's review is counted");
        assert_eq!(mock.count("spawn_mini_coder"), 2, "both ran in the batch");
    }

    #[tokio::test]
    async fn stall_on_a_pre_blocked_predecessor_is_reported() {
        // T001 already blocked, T002 depends on it: no ready task, not all finished -> a
        // stall report (not an infinite loop, not a false success). Nothing delegated.
        let mock = KanbanMock::new(
            vec![
                ktask("T001", "p1", &[], STATUS_BLOCKED, "2026-06-17T10:00:00Z"),
                ktask("T002", "p1", &["T001"], STATUS_TODO, "2026-06-17T10:00:01Z"),
            ],
            &[],
        );
        let report = run_tasks(&mock, "proj", &disabled()).await.unwrap();
        let blocked = report.blocked.expect("a block");
        assert_eq!(blocked.id, "T001", "names the blocked predecessor");
        assert_eq!(
            mock.count("spawn_mini_coder"),
            0,
            "nothing delegated when stalled"
        );
    }

    #[tokio::test]
    async fn stall_names_the_blocked_root_cause_not_a_waiter() {
        // The blocked root cause may not be first by board order: order the waiter (T002)
        // before the blocked T001; the stall must still name T001 (the actionable cause).
        let mock = KanbanMock::new(
            vec![
                ktask("T002", "p1", &["T001"], STATUS_TODO, "2026-06-17T10:00:01Z"),
                ktask("T001", "p1", &[], STATUS_BLOCKED, "2026-06-17T10:00:00Z"),
            ],
            &[],
        );
        let report = run_tasks(&mock, "proj", &disabled()).await.unwrap();
        let blocked = report.blocked.expect("a block");
        assert_eq!(
            blocked.id, "T001",
            "names the blocked root cause, not the waiter T002"
        );
        assert_eq!(mock.count("spawn_mini_coder"), 0);
    }

    #[tokio::test]
    async fn transport_error_blocks_the_task_without_panicking() {
        let mock = MiniErrMock {
            tasks: vec![ktask(
                "T001",
                "p1",
                &[],
                STATUS_TODO,
                "2026-06-17T10:00:00Z",
            )],
        };
        let report = run_tasks(&mock, "proj", &disabled()).await.unwrap();
        let blocked = report.blocked.expect("a block");
        assert_eq!(blocked.id, "T001");
        assert!(
            blocked.reason.contains("spawn_mini_coder failed"),
            "reason: {}",
            blocked.reason
        );
    }

    #[tokio::test]
    async fn claim_then_review_uses_the_expected_params() {
        // Pin the exact tool sequence + params for one task so the Kanban contract is
        // recorded: project_get -> project_claim_task -> spawn_mini_coder ->
        // project_update_status(review) -> project_get (loop) -> (AllDone).
        let mock = KanbanMock::new(
            vec![ktask(
                "T001",
                "p1",
                &[],
                STATUS_TODO,
                "2026-06-17T10:00:00Z",
            )],
            &["done"],
        );
        run_tasks(&mock, "proj", &disabled()).await.unwrap();
        let names = mock.call_names();
        // The claim happens before the delegation, the status update after it.
        let claim = names
            .iter()
            .position(|n| n == "project_claim_task")
            .unwrap();
        let spawn = names.iter().position(|n| n == "spawn_mini_coder").unwrap();
        let upd = names
            .iter()
            .position(|n| n == "project_update_status")
            .unwrap();
        assert!(
            claim < spawn && spawn < upd,
            "claim -> delegate -> update order: {names:?}"
        );
        // The update is a review with project_id + task_id.
        let updates = mock.status_updates();
        assert_eq!(
            updates,
            vec![("T001".to_string(), STATUS_REVIEW.to_string())]
        );
    }

    #[tokio::test]
    async fn empty_project_id_is_an_error() {
        let mock = KanbanMock::new(vec![], &[]);
        let err = run_tasks(&mock, "  ", &disabled())
            .await
            .expect_err("empty project_id");
        assert!(err.contains("project_id"), "got: {err}");
        assert_eq!(mock.count("project_get"), 0, "no read attempted");
    }

    #[tokio::test]
    async fn missing_state_tasks_is_a_clear_error() {
        // A project_get without a state.tasks array is a clear error (not a silent no-op).
        struct BadMock;
        #[async_trait]
        impl McpBackend for BadMock {
            async fn call_tool(&self, name: &str, _p: serde_json::Value) -> Result<String, String> {
                match name {
                    "project_get" => {
                        Ok(serde_json::json!({"metadata": {"id": "proj"}}).to_string())
                    }
                    other => Err(format!("unexpected {other}")),
                }
            }
        }
        let err = run_tasks(&BadMock, "proj", &disabled())
            .await
            .expect_err("no state.tasks");
        assert!(err.contains("state.tasks"), "got: {err}");
    }

    // --- In-run attempt cap --------------------------------------------------

    #[tokio::test]
    async fn a_wip_task_that_finishes_on_first_attempt_is_fine() {
        // A task that starts `wip` (an interrupted prior run) whose mini returns `done` on
        // the first attempt must succeed — the IN-RUN cap does not fire for a clean run.
        let mock = KanbanMock::new(
            vec![ktask("T001", "p1", &[], STATUS_WIP, "2026-06-17T10:00:00Z")],
            &["done"],
        );
        let report = run_tasks(&mock, "proj", &disabled()).await.unwrap();
        assert_eq!(
            report.blocked, None,
            "a wip task is re-attemptable and can finish"
        );
        assert_eq!((report.completed, report.total), (1, 1));
        assert_eq!(mock.status_of("T001"), STATUS_REVIEW);
    }

    // N2: deterministic test that drives a task exactly to the attempts cap.
    // A mock whose `project_get` always reports the task as `todo` (simulating a
    // hostile concurrent mutator that keeps flipping it back) while `spawn_mini_coder`
    // always returns `done` and `project_update_status` succeeds. The runner will
    // attempt the task MAX_TASK_ATTEMPTS times and then block it on the
    // (MAX_TASK_ATTEMPTS+1)th read — never running a third delegation.
    #[tokio::test]
    async fn task_is_blocked_after_exactly_max_task_attempts_delegations() {
        /// A mock that freezes the task board: `project_get` always returns the task as
        /// `todo` regardless of any `project_update_status` calls, so the runner sees it
        /// as runnable on every iteration and keeps re-attempting it until the cap fires.
        struct FrozenBoardMock {
            calls: Mutex<Vec<String>>,
        }
        impl FrozenBoardMock {
            fn new() -> Self {
                Self {
                    calls: Mutex::new(Vec::new()),
                }
            }
            fn spawn_count(&self) -> usize {
                self.calls
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|n| n.as_str() == "spawn_mini_coder")
                    .count()
            }
        }
        #[async_trait]
        impl McpBackend for FrozenBoardMock {
            async fn call_tool(
                &self,
                name: &str,
                _params: serde_json::Value,
            ) -> Result<String, String> {
                self.calls.lock().unwrap().push(name.to_string());
                match name {
                    "project_get" => Ok(serde_json::json!({
                        "metadata": {"id": "proj"},
                        "state": {
                            "tasks": [ktask(
                                "T001", "p1", &[], STATUS_TODO, "2026-06-17T10:00:00Z"
                            )],
                        },
                    })
                    .to_string()),
                    "project_claim_task" => Ok(r#"{"claimed":"T001"}"#.to_string()),
                    "project_update_status" => {
                        // Succeeds but does NOT mutate any state — so project_get always
                        // returns todo, keeping the task perpetually "runnable".
                        Ok(r#"{"metadata":{"id":"proj"}}"#.to_string())
                    }
                    "spawn_mini_coder" | "spawn_main_coder" => Ok(
                        serde_json::json!({"directiveId":"d","result":{"status":"done"}})
                            .to_string(),
                    ),
                    other => Err(format!("unexpected tool {other}")),
                }
            }
        }

        let mock = FrozenBoardMock::new();
        let report = run_tasks(&mock, "proj", &disabled()).await.unwrap();

        // The runner must have stopped with a block (the attempts cap).
        let blocked = report.blocked.expect("cap must fire");
        assert_eq!(blocked.id, "T001");
        assert!(
            blocked.reason.contains("exceeded"),
            "reason names the cap: {}",
            blocked.reason
        );
        assert!(
            blocked.reason.contains(&MAX_TASK_ATTEMPTS.to_string()),
            "reason names MAX_TASK_ATTEMPTS: {}",
            blocked.reason
        );

        // EXACTLY MAX_TASK_ATTEMPTS delegations ran before the cap fired (not MAX+1).
        assert_eq!(
            mock.spawn_count(),
            MAX_TASK_ATTEMPTS as usize,
            "exactly MAX_TASK_ATTEMPTS={MAX_TASK_ATTEMPTS} delegations before cap"
        );
    }
}
