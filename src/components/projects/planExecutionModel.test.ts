// Pure-model unit tests for planExecutionModel.ts.
// Runs in the NODE vitest environment (no jsdom, no DOM).
import { describe, expect, it } from "vitest";
import type { ProjectTask, ProjectTaskStatus } from "../../types/backend";
import {
  buildDepLabel,
  buildPlanExecutionModel,
  filterPlanTasks,
  isTerminalStatus,
  selectActivePlanId,
  statusGlyph,
} from "./planExecutionModel";

// ---- fixtures ---------------------------------------------------------------

function task(
  over: Partial<ProjectTask> & { id: string; status: ProjectTaskStatus },
): ProjectTask {
  return {
    title: `Task ${over.id}`,
    priority: null,
    assignee: null,
    due: null,
    linkedResources: [],
    updatedAt: "2026-06-15T10:00:00Z",
    suspectFileIds: [],
    ...over,
  };
}

// A mixed list: some plan-tagged, some not, varied statuses, two plan ids.
const PLAN_A = "plan-alpha";
const PLAN_B = "plan-beta";

const TASKS: ProjectTask[] = [
  // Plan A — in progress (has a non-terminal task)
  task({ id: "T1", status: "done", planId: PLAN_A, updatedAt: "2026-06-14T09:00:00Z" }),
  task({ id: "T2", status: "review", planId: PLAN_A, updatedAt: "2026-06-14T10:00:00Z" }),
  task({ id: "T3", status: "wip", planId: PLAN_A, dependsOn: ["T1", "T2"], updatedAt: "2026-06-14T11:00:00Z" }),
  task({ id: "T4", status: "todo", planId: PLAN_A, dependsOn: ["T3"], updatedAt: "2026-06-14T08:00:00Z" }),
  // Plan B — all terminal (done or review)
  task({ id: "T5", status: "done", planId: PLAN_B, updatedAt: "2026-06-13T10:00:00Z" }),
  task({ id: "T6", status: "review", planId: PLAN_B, updatedAt: "2026-06-13T11:00:00Z" }),
  // Non-plan tasks — must be ignored
  task({ id: "T7", status: "todo" }),
  task({ id: "T8", status: "wip" }),
];

// ---- filterPlanTasks --------------------------------------------------------

describe("filterPlanTasks", () => {
  it("returns only tasks with a non-empty planId", () => {
    const result = filterPlanTasks(TASKS);
    expect(result.map((t) => t.id)).toEqual(["T1", "T2", "T3", "T4", "T5", "T6"]);
  });

  it("returns empty array when no tasks have a planId", () => {
    expect(filterPlanTasks([task({ id: "X", status: "todo" })])).toEqual([]);
  });

  it("returns empty array for empty input", () => {
    expect(filterPlanTasks([])).toEqual([]);
  });

  it("excludes tasks with an empty string planId", () => {
    const t = task({ id: "Y", status: "todo", planId: "" });
    expect(filterPlanTasks([t])).toEqual([]);
  });
});

// ---- isTerminalStatus -------------------------------------------------------

describe("isTerminalStatus", () => {
  it("marks review as terminal", () => expect(isTerminalStatus("review")).toBe(true));
  it("marks done as terminal", () => expect(isTerminalStatus("done")).toBe(true));
  it("marks todo as non-terminal", () => expect(isTerminalStatus("todo")).toBe(false));
  it("marks wip as non-terminal", () => expect(isTerminalStatus("wip")).toBe(false));
  it("marks blocked as non-terminal", () => expect(isTerminalStatus("blocked")).toBe(false));
});

// ---- statusGlyph ------------------------------------------------------------

describe("statusGlyph", () => {
  it("todo → ⏳", () => expect(statusGlyph("todo")).toBe("⏳"));
  it("wip → 🔵", () => expect(statusGlyph("wip")).toBe("🔵"));
  it("review → 🟡", () => expect(statusGlyph("review")).toBe("🟡"));
  it("blocked → 🔴", () => expect(statusGlyph("blocked")).toBe("🔴"));
  it("done → ✅", () => expect(statusGlyph("done")).toBe("✅"));
});

// ---- buildDepLabel ----------------------------------------------------------

describe("buildDepLabel", () => {
  it("returns empty string for undefined", () => expect(buildDepLabel(undefined)).toBe(""));
  it("returns empty string for empty array", () => expect(buildDepLabel([])).toBe(""));
  it("formats single dep", () => expect(buildDepLabel(["T1"])).toBe("dep: T1"));
  it("formats multiple deps", () => expect(buildDepLabel(["T1", "T2"])).toBe("dep: T1, T2"));
});

// ---- selectActivePlanId -----------------------------------------------------

describe("selectActivePlanId", () => {
  it("returns null for empty input", () => {
    expect(selectActivePlanId([])).toBeNull();
  });

  it("prefers the in-progress plan (Plan A) over the all-terminal plan (Plan B)", () => {
    const planTasks = filterPlanTasks(TASKS);
    expect(selectActivePlanId(planTasks)).toBe(PLAN_A);
  });

  it("returns the only planId when there is one group", () => {
    const tasks = filterPlanTasks([
      task({ id: "T1", status: "todo", planId: "solo" }),
      task({ id: "T2", status: "wip", planId: "solo" }),
    ]);
    expect(selectActivePlanId(tasks)).toBe("solo");
  });

  it("returns the all-finished plan when ALL groups are terminal (most recent updatedAt)", () => {
    const tasks: ProjectTask[] = [
      task({ id: "A1", status: "done", planId: "old-plan", updatedAt: "2026-06-12T10:00:00Z" }),
      task({ id: "B1", status: "review", planId: "new-plan", updatedAt: "2026-06-15T10:00:00Z" }),
    ];
    // Both are terminal; new-plan has the latest updatedAt.
    expect(selectActivePlanId(tasks)).toBe("new-plan");
  });

  it("among multiple in-progress plans picks the one with the latest updatedAt", () => {
    const tasks: ProjectTask[] = [
      task({ id: "A1", status: "todo", planId: "plan-early", updatedAt: "2026-06-10T10:00:00Z" }),
      task({ id: "B1", status: "wip", planId: "plan-late", updatedAt: "2026-06-16T10:00:00Z" }),
    ];
    expect(selectActivePlanId(tasks)).toBe("plan-late");
  });

  it("tie-break on equal updatedAt: picks lexicographically GREATER planId (matches Rust runner)", () => {
    // Both plans have identical updatedAt — the Rust runner selects the lexicographically
    // greater planId via .then_with(|| a_id.cmp(b_id)) inside max_by.
    const sameTs = "2026-06-16T12:00:00Z";
    const tasks: ProjectTask[] = [
      task({ id: "A1", status: "todo", planId: "plan-aardvark", updatedAt: sameTs }),
      task({ id: "B1", status: "todo", planId: "plan-zebra", updatedAt: sameTs }),
    ];
    // "plan-zebra" > "plan-aardvark" lexicographically → must win.
    expect(selectActivePlanId(tasks)).toBe("plan-zebra");
  });

  it("tie-break: insertion order does not determine the winner (reversed insertion)", () => {
    // Ensure the sort is not accidentally insertion-order-sensitive.
    const sameTs = "2026-06-16T12:00:00Z";
    const tasks: ProjectTask[] = [
      task({ id: "B1", status: "todo", planId: "plan-zebra", updatedAt: sameTs }),
      task({ id: "A1", status: "todo", planId: "plan-aardvark", updatedAt: sameTs }),
    ];
    expect(selectActivePlanId(tasks)).toBe("plan-zebra");
  });
});

// ---- buildPlanExecutionModel ------------------------------------------------

describe("buildPlanExecutionModel", () => {
  it("returns a null activePlanId model when there are no plan tasks", () => {
    const model = buildPlanExecutionModel([
      task({ id: "X", status: "todo" }),
      task({ id: "Y", status: "wip" }),
    ]);
    expect(model.activePlanId).toBeNull();
    expect(model.rows).toHaveLength(0);
    expect(model.doneCount).toBe(0);
    expect(model.totalCount).toBe(0);
  });

  it("returns correct rows for the active plan only (Plan A from TASKS)", () => {
    const model = buildPlanExecutionModel(TASKS);
    expect(model.activePlanId).toBe(PLAN_A);
    // Plan A has T1, T2, T3, T4 in that order.
    expect(model.rows.map((r) => r.id)).toEqual(["T1", "T2", "T3", "T4"]);
  });

  it("attaches correct glyphs to each row", () => {
    const model = buildPlanExecutionModel(TASKS);
    const glyphs = model.rows.map((r) => r.glyph);
    expect(glyphs).toEqual(["✅", "🟡", "🔵", "⏳"]);
  });

  it("attaches dep labels only to tasks with dependsOn", () => {
    const model = buildPlanExecutionModel(TASKS);
    expect(model.rows[0].depLabel).toBe("");          // T1: no deps
    expect(model.rows[1].depLabel).toBe("");          // T2: no deps
    expect(model.rows[2].depLabel).toBe("dep: T1, T2"); // T3
    expect(model.rows[3].depLabel).toBe("dep: T3");  // T4
  });

  it("counts done (review|done) tasks correctly", () => {
    const model = buildPlanExecutionModel(TASKS);
    // T1 = done, T2 = review → doneCount = 2; T3 = wip, T4 = todo
    expect(model.doneCount).toBe(2);
    expect(model.totalCount).toBe(4);
  });

  it("shows all tasks as done when the entire plan is terminal", () => {
    const allDone = [
      task({ id: "D1", status: "done", planId: "finished" }),
      task({ id: "D2", status: "review", planId: "finished" }),
    ];
    const model = buildPlanExecutionModel(allDone);
    expect(model.activePlanId).toBe("finished");
    expect(model.doneCount).toBe(2);
    expect(model.totalCount).toBe(2);
  });

  it("handles blocked tasks and attaches the correct glyph", () => {
    const tasks = [
      task({ id: "B1", status: "blocked", planId: "p", dependsOn: ["X1"] }),
      task({ id: "B2", status: "todo", planId: "p" }),
    ];
    const model = buildPlanExecutionModel(tasks);
    expect(model.rows[0].glyph).toBe("🔴");
    expect(model.rows[0].depLabel).toBe("dep: X1");
  });

  it("returns empty model for completely empty task list", () => {
    const model = buildPlanExecutionModel([]);
    expect(model.activePlanId).toBeNull();
    expect(model.rows).toHaveLength(0);
  });
});
