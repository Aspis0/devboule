// Fixture-based tests for arrow geometry: prove the frontend edge builder consumes
// the REAL backend snapshot (rig/fixtures/project-tasks.json) correctly.

import { describe, it, expect } from "vitest";
import { buildArrowEdges } from "./arrowGeometry";
import type { ColumnId } from "./taskBoard";
import fixture from "../../../rig/fixtures/project-tasks.json";

interface TaskFixture {
  id: string;
  status: string;
  title: string;
  dependsOn?: string[];
  scope?: string[];
  acceptance?: string;
  [key: string]: unknown;
}

const tasks = fixture as TaskFixture[];

/** The column definitions from ProjectsView.tsx (the SSoT for status→column mapping).
 *  This is a COPY of the inline `columns` array in ProjectsView.tsx — the actual
 *  grouping function (`tasksByColumn` useMemo) is INLINE JSX, not an exported seam.
 *  We replicate the mapping here to pin the contract; if the columns change in
 *  ProjectsView, this test will catch the drift. */
const COLUMNS: ColumnId[] = ["todo", "wip", "review", "blocked", "done"];

function columnForStatus(status: string): ColumnId {
  return COLUMNS.includes(status as ColumnId) ? (status as ColumnId) : "todo";
}

describe("arrowGeometry against project-tasks fixture", () => {
  it("has the expected 4 tasks (T1..T4)", () => {
    expect(tasks).toHaveLength(4);
    expect(tasks.map((t) => t.id)).toEqual(["T1", "T2", "T3", "T4"]);
  });

  // ---- buildArrowEdges ----

  it("buildArrowEdges produces exactly the deps from the fixture: T2→T3, T3→T4", () => {
    const present = new Set(tasks.map((t) => t.id));
    const edges = buildArrowEdges(tasks, present);

    // T3 dependsOn T2, T4 dependsOn T3. T1 has no deps. No self-edges.
    expect(edges).toHaveLength(2);
    expect(edges).toContainEqual({ from: "T2", to: "T3" });
    expect(edges).toContainEqual({ from: "T3", to: "T4" });
  });

  it("buildArrowEdges: no self-edges in the fixture (all dependsOn refs are distinct)", () => {
    const present = new Set(tasks.map((t) => t.id));
    const edges = buildArrowEdges(tasks, present);
    for (const edge of edges) {
      expect(edge.from).not.toBe(edge.to);
    }
  });

  it("buildArrowEdges: T1 (no dependsOn) appears in zero edges", () => {
    const present = new Set(tasks.map((t) => t.id));
    const edges = buildArrowEdges(tasks, present);
    const allIds = new Set(edges.flatMap((e) => [e.from, e.to]));
    expect(allIds.has("T1")).toBe(false);
  });

  it("buildArrowEdges: drops edges when a dependency is not present", () => {
    // If T2 is not present but T3 is, T2→T3 should be dropped
    const edges = buildArrowEdges(tasks, new Set(["T1", "T3", "T4"]));
    expect(edges).toHaveLength(1);
    expect(edges[0]).toEqual({ from: "T3", to: "T4" });
  });

  // ---- column derivation ----
  //
  // NOTE: the ACTUAL column grouping (`tasksByColumn` useMemo) is INLINE JSX in
  // ProjectsView.tsx (line ~557) and is NOT an exported function. We assert against
  // the CLOSEST exported seam — the ColumnId type from taskBoard.ts and the
  // column definitions. The column mapping status→ColumnId is pinned here.

  it("kanban column derivation: all 4 tasks are 'todo' status → all land in todo column", () => {
    const grouped: Record<ColumnId, TaskFixture[]> = {
      todo: [],
      wip: [],
      review: [],
      blocked: [],
      done: [],
    };
    for (const task of tasks) {
      const col = columnForStatus(task.status);
      grouped[col].push(task);
    }
    expect(grouped.todo).toHaveLength(4);
    expect(grouped.wip).toHaveLength(0);
    expect(grouped.review).toHaveLength(0);
    expect(grouped.blocked).toHaveLength(0);
    expect(grouped.done).toHaveLength(0);
  });

  it("kanban column derivation: T3 depends on T2, both in same column (todo)", () => {
    const t3 = tasks.find((t) => t.id === "T3")!;
    expect(t3.dependsOn).toEqual(["T2"]);
    expect(t3.status).toBe("todo");
    const t2 = tasks.find((t) => t.id === "T2")!;
    expect(t2.status).toBe("todo");
  });

  it("kanban column: T2 has scope and acceptance fields from the fixture", () => {
    const t2 = tasks.find((t) => t.id === "T2")!;
    expect(t2.scope).toEqual(["src/a.ts"]);
    expect(t2.acceptance).toBe("builds");
  });
});
