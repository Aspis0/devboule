import { describe, it, expect } from "vitest";
import {
  rowBadges,
  rowActions,
  drawerData,
  subagentChipLabel,
  buildLaunchInput,
  normalizeModelHint,
  modelSuggestionsForClient,
  MODEL_HINT_MAX_LENGTH,
  spawnDisabledReason,
  canRoleLaunchTask,
  formatStamp,
  fleetHealthRollup,
  capHistory,
} from "./agentRowModel";
import type {
  AgentClaim,
  AgentEvent,
  AgentSession,
  ProjectTask,
} from "../../types/backend";

// Pure-logic tests for the rebuilt fleet view. No DOM: the .tsx shells only map
// these structs to JSX, so the badge/drawer/spawn derivations are the contract.

const NOW = Date.parse("2026-06-04T10:00:00.000Z");
const FRESH = "2026-06-04T09:59:50.000Z"; // 10s ago -> online
const STALE = "2026-06-04T09:55:00.000Z"; // 5m ago -> stale
const LOST = "2026-06-04T09:45:00.000Z"; // 15m ago -> lost

function session(overrides: Partial<AgentSession>): AgentSession {
  return {
    agentId: "a-1",
    role: "coder",
    model: "sonnet",
    status: "wip",
    message: null,
    currentProjectId: null,
    currentTaskId: null,
    firstSeenAt: FRESH,
    lastSeenAt: FRESH,
    ...overrides,
  };
}

function claim(overrides: Partial<AgentClaim>): AgentClaim {
  return {
    projectId: "p-1",
    projectTitle: "Proj",
    taskId: "t-1",
    taskTitle: "Task",
    agentId: "a-1",
    role: "coder",
    status: "wip",
    claimedAt: FRESH,
    updatedAt: FRESH,
    leaseUntil: "2026-06-04T11:00:00.000Z",
    evidence: null,
    ...overrides,
  };
}

function event(overrides: Partial<AgentEvent>): AgentEvent {
  return {
    id: "e-1",
    timestamp: FRESH,
    agentId: "a-1",
    role: "coder",
    eventType: "status",
    projectId: "p-1",
    taskId: "t-1",
    status: null,
    message: "hi",
    evidence: null,
    ...overrides,
  };
}

function task(overrides: Partial<ProjectTask>): ProjectTask {
  return {
    id: "t-1",
    title: "Task",
    status: "todo",
    priority: "medium",
    assignee: null,
    due: null,
    linkedResources: [],
    updatedAt: FRESH,
    suspectFileIds: [],
    ...overrides,
  };
}

describe("rowBadges", () => {
  it("reports online health and a known model", () => {
    const b = rowBadges(session({ model: "opus" }), NOW);
    expect(b.health).toBe("online");
    expect(b.modelKnown).toBe(true);
    expect(b.modelLabel).toBe("opus");
    expect(b.recovery).toBe(false);
  });

  it("falls back to 'model unknown' and flags recovery on a lost heartbeat", () => {
    const b = rowBadges(session({ model: null, lastSeenAt: LOST }), NOW);
    expect(b.modelKnown).toBe(false);
    expect(b.modelLabel).toBe("model unknown");
    expect(b.health).toBe("lost");
    expect(b.recovery).toBe(true);
  });

  it("builds pluralized subagent chips and a headcount total", () => {
    const b = rowBadges(
      session({
        subagents: [
          { label: "reviewers", model: "sonnet", count: 6, role: "reviewer" },
          { label: "scratch", model: "haiku", count: 1 },
        ],
      }),
      NOW,
    );
    expect(b.subagentChips).toEqual(["+6 sonnet reviewer", "+1 haiku"]);
    expect(b.subagentTotal).toBe(7);
  });

  it("surfaces the needsUser message preview, falling back to reason", () => {
    expect(
      rowBadges(
        session({
          needsUser: {
            reason: "permission",
            message: "  Allow deploy? ",
            since: FRESH,
          },
        }),
        NOW,
      ).needsUserMessage,
    ).toBe("Allow deploy?");
    expect(
      rowBadges(
        session({
          needsUser: { reason: "blocked", message: "   ", since: FRESH },
        }),
        NOW,
      ).needsUserMessage,
    ).toBe("blocked");
    expect(rowBadges(session({}), NOW).needsUserMessage).toBeNull();
  });
});

describe("subagentChipLabel", () => {
  it("degrades to '+n model' when role is absent", () => {
    expect(subagentChipLabel({ label: "x", model: "opus", count: 3 })).toBe(
      "+3 opus",
    );
  });
  it("uses 'unknown' for a blank model", () => {
    expect(subagentChipLabel({ label: "x", model: "", count: 2 })).toBe(
      "+2 unknown",
    );
  });
});

describe("drawerData", () => {
  it("filters claims/events to one agent and splits by lifecycle", () => {
    const claims = [
      claim({ agentId: "a-1", status: "wip", taskId: "t-1" }),
      claim({ agentId: "a-1", status: "review", taskId: "t-2" }),
      claim({ agentId: "a-1", status: "done", taskId: "t-3" }),
      claim({ agentId: "other", status: "wip", taskId: "t-9" }),
    ];
    const events = [
      event({ id: "e-1", agentId: "a-1", timestamp: STALE }),
      event({ id: "e-2", agentId: "a-1", timestamp: FRESH }),
      event({ id: "e-3", agentId: "other" }),
    ];
    const d = drawerData(session({ agentId: "a-1" }), claims, events, NOW);
    expect(d.activeClaims.map((c) => c.taskId)).toEqual(["t-1"]);
    expect(d.waitingClaims.map((c) => c.taskId)).toEqual(["t-2"]);
    expect(d.historyClaims.map((c) => c.taskId)).toEqual(["t-3"]);
    // Events newest-first, other agent excluded.
    expect(d.events.map((e) => e.id)).toEqual(["e-2", "e-1"]);
  });

  it("does not throw on null timestamps/updatedAt", () => {
    const d = drawerData(
      session({ agentId: "a-1" }),
      [claim({ updatedAt: undefined as unknown as string })],
      [event({ timestamp: undefined as unknown as string })],
      NOW,
    );
    expect(d.events).toHaveLength(1);
  });
});

describe("normalizeModelHint", () => {
  it("returns null for blank/custom and lowercases otherwise", () => {
    expect(normalizeModelHint("")).toBeNull();
    expect(normalizeModelHint("   ")).toBeNull();
    // The UI no longer emits the "custom" literal, but it is still mapped to null
    // defensively for a stale caller/value.
    expect(normalizeModelHint("  custom ")).toBeNull();
    expect(normalizeModelHint("CUSTOM")).toBeNull();
    expect(normalizeModelHint("Opus")).toBe("opus");
    expect(normalizeModelHint(" my-llm ")).toBe("my-llm");
    // An arbitrary self-hosted model name rides along verbatim (lowercased).
    expect(normalizeModelHint("DeepSeek-V3")).toBe("deepseek-v3");
  });

  it("caps an over-long hint to MODEL_HINT_MAX_LENGTH characters", () => {
    const long = "x".repeat(MODEL_HINT_MAX_LENGTH + 50);
    const out = normalizeModelHint(long);
    expect(out).toHaveLength(MODEL_HINT_MAX_LENGTH);
    expect(out).toBe("x".repeat(MODEL_HINT_MAX_LENGTH));
  });
});

describe("modelSuggestionsForClient", () => {
  it("offers opus/sonnet/haiku only for the claude CLI", () => {
    expect(modelSuggestionsForClient("claude")).toEqual([
      "opus",
      "sonnet",
      "haiku",
    ]);
  });

  it("invents no model names for codex, powershell or any custom client", () => {
    expect(modelSuggestionsForClient("codex")).toEqual([]);
    expect(modelSuggestionsForClient("powershell")).toEqual([]);
    expect(modelSuggestionsForClient("deepseek")).toEqual([]);
    expect(modelSuggestionsForClient("")).toEqual([]);
  });
});

describe("buildLaunchInput", () => {
  const sel = {
    projectId: "p-1",
    role: "coder" as const,
    model: "opus",
    taskId: "t-1",
    client: "codex" as const,
  };

  it("threads host=app and a normalized model into the IPC input", () => {
    expect(buildLaunchInput(sel, "app")).toEqual({
      projectId: "p-1",
      role: "coder",
      client: "codex",
      taskId: "t-1",
      host: "app",
      model: "opus",
    });
  });

  it("threads host=external and drops a blank task to null", () => {
    expect(buildLaunchInput({ ...sel, taskId: "  ", model: "" }, "external")).toEqual(
      {
        projectId: "p-1",
        role: "coder",
        client: "codex",
        taskId: null,
        host: "external",
        model: null,
      },
    );
  });

  it("threads a configured custom client id through verbatim", () => {
    const input = buildLaunchInput(
      { ...sel, client: "deepseek", model: "deepseek-v3" },
      "app",
    );
    expect(input.client).toBe("deepseek");
    expect(input.model).toBe("deepseek-v3");
    expect(input.host).toBe("app");
  });

  it("throws for host=copy (not a launch path)", () => {
    expect(() => buildLaunchInput(sel, "copy")).toThrow();
  });

  it("leaves censorReview unset for a normal SpawnPanel launch (back-compat)", () => {
    // Only the Censor "Run final review" path sets censorReview; a normal spawn
    // must not, so the backend's lenient default keeps the verifier prompt
    // unchanged.
    expect(buildLaunchInput(sel, "app").censorReview).toBeUndefined();
  });
});

describe("canRoleLaunchTask / spawnDisabledReason", () => {
  it("blocks coder on review tasks and verifier on todo tasks", () => {
    expect(canRoleLaunchTask("coder", task({ status: "review" }))).toBe(false);
    expect(canRoleLaunchTask("verifier", task({ status: "todo" }))).toBe(false);
    expect(canRoleLaunchTask("verifier", task({ status: "review" }))).toBe(true);
    expect(canRoleLaunchTask("coder", task({ status: "wip" }))).toBe(true);
  });

  it("explains why a spawn is disabled, in precedence order", () => {
    expect(
      spawnDisabledReason({
        projectId: "all",
        projectActive: true,
        role: "coder",
        task: null,
      }),
    ).toMatch(/Select a project/);
    expect(
      spawnDisabledReason({
        projectId: "p-1",
        projectActive: null,
        role: "coder",
        task: null,
      }),
    ).toMatch(/loading/i);
    expect(
      spawnDisabledReason({
        projectId: "p-1",
        projectActive: false,
        role: "coder",
        task: null,
      }),
    ).toMatch(/active projects/);
    expect(
      spawnDisabledReason({
        projectId: "p-1",
        projectActive: true,
        role: "verifier",
        task: task({ status: "todo" }),
      }),
    ).toMatch(/Verifier can launch only/);
    expect(
      spawnDisabledReason({
        projectId: "p-1",
        projectActive: true,
        role: "coder",
        task: task({ status: "wip" }),
      }),
    ).toBeNull();
  });
});

describe("rowActions", () => {
  it("app host with a live PTY: terminal toggle, no Open CLI, no exited hint", () => {
    const a = rowActions(session({ host: "app" }), true);
    expect(a).toEqual({
      showTerminalToggle: true,
      showOpenCli: false,
      showExitedHint: false,
    });
  });

  it("app host whose PTY exited: no toggle, no Open CLI, exited hint", () => {
    const a = rowActions(session({ host: "app" }), false);
    expect(a).toEqual({
      showTerminalToggle: false,
      showOpenCli: false,
      showExitedHint: true,
    });
  });

  it("external host: Open CLI, no terminal toggle, no exited hint", () => {
    const a = rowActions(session({ host: "external" }), false);
    expect(a).toEqual({
      showTerminalToggle: false,
      showOpenCli: true,
      showExitedHint: false,
    });
  });

  it("no host (not app-launched): Open CLI stays, no exited hint even without a PTY", () => {
    const a = rowActions(session({ host: null }), false);
    expect(a).toEqual({
      showTerminalToggle: false,
      showOpenCli: true,
      showExitedHint: false,
    });
    // host absent (undefined) behaves the same as null.
    expect(rowActions(session({}), false).showOpenCli).toBe(true);
    expect(rowActions(session({}), false).showExitedHint).toBe(false);
  });
});

describe("fleetHealthRollup", () => {
  it("buckets online/stale/lost and excludes closed sessions", () => {
    const roll = fleetHealthRollup(
      [
        session({ lastSeenAt: FRESH }), // online
        session({ status: "launch_pending", lastSeenAt: null, firstSeenAt: FRESH }), // pending -> online bucket
        session({ lastSeenAt: STALE }), // stale
        session({ lastSeenAt: LOST }), // lost
        session({ status: "done", lastSeenAt: FRESH }), // closed -> excluded
        // status "closed" (literal written by Rust mark_agent_session_closed):
        // a just-stopped agent with a fresh heartbeat must NOT count as online.
        session({ status: "closed", lastSeenAt: FRESH }),
      ],
      NOW,
    );
    expect(roll).toEqual({ online: 2, stale: 1, lost: 1 });
  });
});

describe("capHistory", () => {
  it("keeps only the most recent `limit` closed sessions, newest first", () => {
    const sessions = [
      session({ agentId: "old", lastSeenAt: LOST }),
      session({ agentId: "new", lastSeenAt: FRESH }),
      session({ agentId: "mid", lastSeenAt: STALE }),
    ];
    const capped = capHistory(sessions, 2);
    expect(capped.sessions.map((s) => s.agentId)).toEqual(["new", "mid"]);
    expect(capped.total).toBe(3);
    expect(capped.truncated).toBe(true);
  });

  it("returns all sessions when under the limit (not truncated)", () => {
    const sessions = [
      session({ agentId: "a", lastSeenAt: FRESH }),
      session({ agentId: "b", lastSeenAt: STALE }),
    ];
    const capped = capHistory(sessions, 20);
    expect(capped.sessions).toHaveLength(2);
    expect(capped.total).toBe(2);
    expect(capped.truncated).toBe(false);
  });

  it("sorts a null lastSeenAt to the end", () => {
    const sessions = [
      session({ agentId: "nullts", lastSeenAt: null }),
      session({ agentId: "has", lastSeenAt: FRESH }),
    ];
    const capped = capHistory(sessions, 20);
    expect(capped.sessions.map((s) => s.agentId)).toEqual(["has", "nullts"]);
  });

  it("does not mutate the input array", () => {
    const sessions = [
      session({ agentId: "old", lastSeenAt: LOST }),
      session({ agentId: "new", lastSeenAt: FRESH }),
    ];
    const before = sessions.map((s) => s.agentId);
    capHistory(sessions, 1);
    expect(sessions.map((s) => s.agentId)).toEqual(before);
  });

  it("handles an empty list and a non-positive limit", () => {
    expect(capHistory([], 20)).toEqual({
      sessions: [],
      total: 0,
      truncated: false,
    });
    const capped = capHistory(
      [session({ agentId: "x" }), session({ agentId: "y" })],
      0,
    );
    expect(capped).toEqual({ sessions: [], total: 2, truncated: true });
  });
});

describe("formatStamp", () => {
  it("returns 'never' for nullish and a string for a valid date", () => {
    expect(formatStamp(null)).toBe("never");
    expect(formatStamp(undefined)).toBe("never");
    expect(typeof formatStamp(FRESH)).toBe("string");
  });
});
