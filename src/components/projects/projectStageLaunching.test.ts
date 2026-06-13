import { describe, it, expect } from "vitest";
import { projectStage } from "./projectStage";
import { LAUNCH_PENDING_STALE_MS } from "./agentLiveStatus";

// BUG #19: a launch_pending agent that never registers (e.g. its MCP server
// could not start) used to keep the PROJECT stuck at "launching" for the full
// 15-min activity window — even though the agent DOT already self-heals to
// "stalled" after LAUNCH_PENDING_STALE_MS (~2 min). The project-stage derivation
// must agree with sessionHealth: a stale launch_pending no longer counts as
// "launching", so the project reverts to "planned" instead of hanging.
describe("projectStage — launch_pending staleness (bug #19)", () => {
  const NOW = 1_700_000_000_000;

  const summary = (overrides: Record<string, unknown> = {}) =>
    ({
      status: "active",
      taskCounts: { todo: 0, wip: 0, review: 0, blocked: 0, done: 0, total: 0 },
      ...overrides,
    }) as Parameters<typeof projectStage>[0];

  const launchPending = (ageMs: number) =>
    ({
      agentId: "a1",
      role: "coder",
      status: "launch_pending",
      currentProjectId: "p1",
      lastSeenAt: new Date(NOW - ageMs).toISOString(),
      firstSeenAt: new Date(NOW - ageMs).toISOString(),
    }) as Parameters<typeof projectStage>[2][number];

  it("a FRESH launch_pending makes the project 'launching'", () => {
    const fresh = launchPending(30 * 1000); // 30s old, < 2 min
    expect(projectStage(summary(), [], [fresh], NOW)).toBe("launching");
  });

  it("a STALE launch_pending no longer counts as 'launching' (reverts to planned)", () => {
    const stale = launchPending(LAUNCH_PENDING_STALE_MS + 60 * 1000); // 3 min old
    expect(projectStage(summary(), [], [stale], NOW)).toBe("planned");
  });

  // Defensive: record_launch_pending always stamps last_seen_at at launch time,
  // so a null heartbeat should not occur — but if it ever does, isRecentProjectSession
  // short-circuits on null lastSeen, so the session is excluded (not "launching").
  it("a launch_pending with a null heartbeat is excluded (planned, not launching)", () => {
    const neverSeen = {
      agentId: "a1",
      role: "coder",
      status: "launch_pending",
      currentProjectId: "p1",
      lastSeenAt: null,
      firstSeenAt: new Date(NOW - 30 * 1000).toISOString(),
    } as Parameters<typeof projectStage>[2][number];
    expect(projectStage(summary(), [], [neverSeen], NOW)).toBe("planned");
  });

  // Boundary: sessionHealth uses `ageMs > LAUNCH_PENDING_STALE_MS` (strict), so
  // AT the threshold the launch is still pending, ONE ms past it goes stale.
  it("is exact at the staleness boundary", () => {
    const atBoundary = launchPending(LAUNCH_PENDING_STALE_MS); // ageMs == threshold
    expect(projectStage(summary(), [], [atBoundary], NOW)).toBe("launching");
    const justPast = launchPending(LAUNCH_PENDING_STALE_MS + 1); // ageMs == threshold + 1ms
    expect(projectStage(summary(), [], [justPast], NOW)).toBe("planned");
  });
});
