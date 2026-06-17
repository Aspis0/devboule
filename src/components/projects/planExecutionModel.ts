// Pure, DOM-free model for the Plan Execution view (UX piece 2).
//
// All selection/filter/formatting logic lives here so PlanExecutionView.tsx is a
// thin JSX shell and the logic is unit-testable in the node vitest env without React.
//
// "Active plan" definition: among the planId groups present in the task list, pick
// the ONE group that contains ≥1 task NOT in {review, done}. If multiple such groups
// exist (should be rare — a single plan per project is the norm), pick the one with
// the most recent task updatedAt as a tiebreaker. If all groups are finished (every
// task is review|done), return the group that appears last by that same tiebreaker
// so the user still sees the completed plan.

import type { ProjectTask, ProjectTaskStatus } from "../../types/backend";

// ---- types ------------------------------------------------------------------

export interface PlanTaskRow {
  id: string;
  title: string;
  status: ProjectTaskStatus;
  /** The dep-label string shown in the row, e.g. "dep: T2, T3". Empty string
   *  when there are no dependencies. */
  depLabel: string;
  /** Canonical status glyph for this status. */
  glyph: string;
}

export interface PlanExecutionModel {
  /** The planId being rendered. Null when no plan-tagged tasks exist. */
  activePlanId: string | null;
  /** The task rows to render, in their original task list order. */
  rows: PlanTaskRow[];
  /** Count of tasks in a terminal state (review or done). */
  doneCount: number;
  /** Total tasks in the plan group. */
  totalCount: number;
}

// ---- status glyphs ----------------------------------------------------------

const STATUS_GLYPH: Record<ProjectTaskStatus, string> = {
  todo: "⏳",
  wip: "🔵",
  review: "🟡",
  blocked: "🔴",
  done: "✅",
};

/** Return the display glyph for a task status. Unknown statuses fall back to ⏳. */
export function statusGlyph(status: ProjectTaskStatus): string {
  return STATUS_GLYPH[status] ?? "⏳";
}

// ---- pure model helpers -----------------------------------------------------

/** Return only the tasks that carry a non-empty planId. Order is preserved. */
export function filterPlanTasks(tasks: ProjectTask[]): ProjectTask[] {
  return tasks.filter(
    (t) => typeof t.planId === "string" && t.planId.trim().length > 0,
  );
}

/** A terminal task is one in {review, done}. Used for done-count and active-plan
 *  selection (a plan is "finished" when all its tasks are terminal). */
export function isTerminalStatus(status: ProjectTaskStatus): boolean {
  return status === "review" || status === "done";
}

/** Pick the active planId from a list of plan-tagged tasks.
 *  Returns null when the list is empty.
 *
 *  Strategy:
 *  1. Group tasks by planId.
 *  2. Prefer a group with ≥1 non-terminal task (the in-progress plan).
 *     When multiple in-progress groups exist, pick the one whose most-recent
 *     updatedAt is latest.
 *  3. If every group is fully terminal, pick the group with the most-recent
 *     updatedAt across its tasks (the "most recently active finished plan"). */
export function selectActivePlanId(planTasks: ProjectTask[]): string | null {
  if (planTasks.length === 0) return null;

  // Build group map: planId → tasks[]
  const groups = new Map<string, ProjectTask[]>();
  for (const task of planTasks) {
    const pid = task.planId as string; // guaranteed non-empty by filterPlanTasks
    const existing = groups.get(pid);
    if (existing) existing.push(task);
    else groups.set(pid, [task]);
  }

  /** Latest ISO updatedAt in a group — used as tiebreaker. */
  function latestUpdatedAt(group: ProjectTask[]): string {
    return group.reduce(
      (best, t) => (t.updatedAt > best ? t.updatedAt : best),
      "",
    );
  }

  /** True when the group has ≥1 non-terminal task. */
  function isInProgress(group: ProjectTask[]): boolean {
    return group.some((t) => !isTerminalStatus(t.status));
  }

  const inProgress = [...groups.entries()].filter(([, g]) => isInProgress(g));

  const candidates = inProgress.length > 0 ? inProgress : [...groups.entries()];

  // Pick the candidate whose most-recent task updatedAt is latest. Tie-break on the
  // planId string (lexicographically GREATER wins) to match the Rust runner's
  // select_active_plan which uses: .max_by(|a, b| a_ts.cmp(b_ts).then_with(|| a_id.cmp(b_id)))
  // — Rust's max_by picks the element that compares Greater, so the larger planId wins.
  let bestId: string | null = null;
  let bestTs = "";
  for (const [pid, group] of candidates) {
    const ts = latestUpdatedAt(group);
    if (ts > bestTs || (ts === bestTs && bestId !== null && pid > bestId)) {
      bestTs = ts;
      bestId = pid;
    }
  }
  return bestId;
}

/** Build the dep label string ("dep: T2, T3") from a dependsOn array.
 *  Returns an empty string when there are no dependencies. */
export function buildDepLabel(dependsOn: string[] | undefined): string {
  if (!dependsOn || dependsOn.length === 0) return "";
  return `dep: ${dependsOn.join(", ")}`;
}

/** Build the complete PlanExecutionModel from a raw task list.
 *  Safe to call with any input (empty, no plan tasks, mixed). */
export function buildPlanExecutionModel(tasks: ProjectTask[]): PlanExecutionModel {
  const planTasks = filterPlanTasks(tasks);
  const activePlanId = selectActivePlanId(planTasks);

  if (activePlanId === null) {
    return { activePlanId: null, rows: [], doneCount: 0, totalCount: 0 };
  }

  // Only the tasks belonging to the active plan, in original order.
  const groupTasks = planTasks.filter((t) => t.planId === activePlanId);

  const rows: PlanTaskRow[] = groupTasks.map((t) => ({
    id: t.id,
    title: t.title,
    status: t.status,
    depLabel: buildDepLabel(t.dependsOn),
    glyph: statusGlyph(t.status),
  }));

  const doneCount = groupTasks.filter((t) => isTerminalStatus(t.status)).length;

  return {
    activePlanId,
    rows,
    doneCount,
    totalCount: groupTasks.length,
  };
}
