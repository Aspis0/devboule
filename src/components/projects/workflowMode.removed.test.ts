import { describe, it, expect } from "vitest";

// Regression guard for the "architect ghost" removal: the MOCK workflow-mode
// subsystem in the Projects page (ProjectModePanel + Architect/Reviewer modes +
// their static mock artifacts) was deleted because it created orchestrator /
// architect naming confusion with the REAL agent-role system (orchestrator /
// coder / verifier via MCP).
//
// These tests assert the mock modules stay gone (importing them must fail) while
// the REAL pieces they used to wrap (the Work-mode agent surface and the kanban
// stage derivation) are untouched and still importable.

const removedModules = [
  "./ProjectModePanel",
  "./ArchitectMode",
  "./ReviewerMode",
  "./CoderMode",
  "./workflowMode",
  "./workflowArtifacts",
  "./workflowMockData",
];

describe("architect-ghost removal", () => {
  it.each(removedModules)(
    "no longer ships the mock workflow module %s",
    async (specifier) => {
      await expect(import(/* @vite-ignore */ specifier)).rejects.toThrow();
    },
  );

  it("keeps the REAL Work-mode agent surface", async () => {
    // The board-mode ProjectAgentPanel was retired; its real per-agent controls
    // (stop / focus CLI / recovery) now live in the Work-mode agent surface. Guard
    // that the rail component is still shipped and importable.
    const mod = await import("./ProjectWorkspaceAgentRail");
    expect(typeof mod.ProjectWorkspaceAgentRail).toBe("function");
  });

  it("keeps the REAL kanban stage derivation untouched", async () => {
    const mod = await import("./projectStage");
    expect(typeof mod.projectStage).toBe("function");
    // The real stage ids must NOT include any workflow-mode "architect" id.
    expect(mod.projectStages.map((stage) => stage.id)).not.toContain(
      "architect",
    );
  });

  it("projectStage still derives real stages (behavioral, not just a typeof)", async () => {
    const { projectStage } = await import("./projectStage");
    const summary = (overrides: Record<string, unknown>) =>
      ({
        status: "active",
        taskCounts: { todo: 0, wip: 0, review: 0, blocked: 0, done: 0, total: 0 },
        ...overrides,
      }) as Parameters<typeof projectStage>[0];

    // The byte-for-byte relocated derivation: a shim re-export with different
    // logic would fail at least one of these.
    expect(projectStage(summary({}), [], [])).toBe("planned");
    expect(projectStage(summary({ status: "done" }), [], [])).toBe("verified");
    expect(projectStage(summary({ status: "archived" }), [], [])).toBe(
      "blocked",
    );
    expect(
      projectStage(
        summary({
          taskCounts: { todo: 1, wip: 1, review: 0, blocked: 0, done: 0, total: 2 },
        }),
        [],
        [],
      ),
    ).toBe("active");
    expect(
      projectStage(
        summary({
          taskCounts: { todo: 1, wip: 0, review: 1, blocked: 0, done: 0, total: 2 },
        }),
        [],
        [],
      ),
    ).toBe("review");
  });
});
