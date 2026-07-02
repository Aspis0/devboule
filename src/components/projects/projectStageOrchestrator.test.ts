import { describe, it, expect } from "vitest";
import { projectStage } from "./projectStage";

// The planner/orchestrator session is the CREATE-TIME conversation, not project
// work: it must never drive the kanban stage. Before this fix, merely talking to
// the planner flipped the project card into "Active"/"Launching" (the user saw
// "the plan goes active by itself when I change page" — the session registered
// while they were away and the stage recomputed on remount).
describe("projectStage — orchestrator sessions are not project work", () => {
  const NOW = 1_700_000_000_000;

  const summary = (overrides: Record<string, unknown> = {}) =>
    ({
      status: "active",
      taskCounts: { todo: 0, wip: 0, review: 0, blocked: 0, done: 0, total: 0 },
      ...overrides,
    }) as Parameters<typeof projectStage>[0];

  const session = (role: string, status: string, ageMs = 30_000) =>
    ({
      agentId: "a1",
      role,
      status,
      currentProjectId: "p1",
      lastSeenAt: new Date(NOW - ageMs).toISOString(),
      firstSeenAt: new Date(NOW - ageMs).toISOString(),
    }) as Parameters<typeof projectStage>[2][number];

  it("an ACTIVE orchestrator session leaves the project in 'planned'", () => {
    expect(projectStage(summary(), [], [session("orchestrator", "active")], NOW)).toBe(
      "planned",
    );
  });

  it("a LAUNCH_PENDING orchestrator session does not make the project 'launching'", () => {
    expect(
      projectStage(summary(), [], [session("orchestrator", "launch_pending")], NOW),
    ).toBe("planned");
  });

  it("a coder session still activates the project as before", () => {
    expect(projectStage(summary(), [], [session("coder", "active")], NOW)).toBe(
      "active",
    );
  });

  it("a coder launch_pending still shows 'launching' alongside an orchestrator chat", () => {
    expect(
      projectStage(
        summary(),
        [],
        [session("orchestrator", "active"), session("coder", "launch_pending")],
        NOW,
      ),
    ).toBe("launching");
  });
});
