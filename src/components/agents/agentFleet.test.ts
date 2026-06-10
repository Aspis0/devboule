import { describe, it, expect } from "vitest";
import {
  fleetCounts,
  summarizeFleet,
  attentionSessions,
  reportedSubagentTotal,
  fleetHeadlineSuffix,
  type FleetCount,
} from "./agentFleet";
import type { AgentSession } from "../../types/backend";

// Pure-logic tests for the fleet aggregation selectors. No DOM: the Agents
// control room renders whatever these produce, so the folding rules, ordering,
// summary string and attention filter are the contract worth pinning.

const NOW = Date.parse("2026-06-04T10:00:00.000Z");
const FRESH = "2026-06-04T09:59:50.000Z"; // 10s ago -> online
const STALE = "2026-06-04T09:55:00.000Z"; // 5m ago -> stale (>3m, <10m)
const LOST = "2026-06-04T09:45:00.000Z"; // 15m ago -> lost (>10m)

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

describe("fleetCounts", () => {
  it("folds a single parent session as (role, model) +1", () => {
    const counts = fleetCounts(
      [session({ role: "orchestrator", model: "opus" })],
      NOW,
    );
    expect(counts).toEqual<FleetCount[]>([
      { role: "orchestrator", model: "opus", count: 1 },
    ]);
  });

  it("folds subagents and falls back to the parent role when entry.role is absent", () => {
    const counts = fleetCounts(
      [
        session({
          role: "orchestrator",
          model: "opus",
          subagents: [
            { label: "coders", model: "sonnet", count: 2, role: "coder" },
            { label: "scratch", model: "haiku", count: 1 }, // role omitted
          ],
        }),
      ],
      NOW,
    );
    expect(counts).toEqual<FleetCount[]>([
      { role: "orchestrator", model: "opus", count: 1 },
      // entry.role absent -> inherits parent role "orchestrator"
      { role: "orchestrator", model: "haiku", count: 1 },
      { role: "coder", model: "sonnet", count: 2 },
    ]);
  });

  it("falls back to 'unknown' model for empty/null parent and subagent models", () => {
    const counts = fleetCounts(
      [
        session({
          role: "coder",
          model: null,
          subagents: [{ label: "x", model: "", count: 1, role: "coder" }],
        }),
      ],
      NOW,
    );
    expect(counts).toEqual<FleetCount[]>([
      { role: "coder", model: "unknown", count: 2 },
    ]);
  });

  it("excludes closed sessions and their subagents", () => {
    const counts = fleetCounts(
      [
        // status "closed" written by the Rust backend
        session({
          role: "coder",
          model: "sonnet",
          status: "closed",
          subagents: [{ label: "y", model: "haiku", count: 3, role: "coder" }],
        }),
        // status "done" (closed per agentLiveStatus.sessionHealth)
        session({ role: "verifier", model: "opus", status: "done" }),
        // a live one survives
        session({ role: "coder", model: "sonnet", status: "wip" }),
      ],
      NOW,
    );
    expect(counts).toEqual<FleetCount[]>([
      { role: "coder", model: "sonnet", count: 1 },
    ]);
  });

  it("keeps role/model with spaces distinct (no key collision)", () => {
    // Subagent role/model are arbitrary wire strings that can contain spaces.
    // A naive `${role} ${model}` key collides: "a b"+"c" vs "a"+"b c" both
    // stringify to "a b c". Distinct (role, model) pairs must stay separate.
    const counts = fleetCounts(
      [
        session({
          agentId: "p",
          role: "orchestrator",
          model: "opus",
          subagents: [
            { label: "x", model: "c", count: 1, role: "a b" },
            { label: "y", model: "b c", count: 1, role: "a" },
          ],
        }),
      ],
      NOW,
    );
    const ab = counts.find((c) => c.role === "a b" && c.model === "c");
    const a = counts.find((c) => c.role === "a" && c.model === "b c");
    expect(ab?.count).toBe(1);
    expect(a?.count).toBe(1);
  });

  it("silently drops a subagent entry with count 0 (old pre-clamp files)", () => {
    const counts = fleetCounts(
      [
        session({
          agentId: "p",
          role: "orchestrator",
          model: "opus",
          subagents: [{ label: "z", model: "sonnet", count: 0, role: "coder" }],
        }),
      ],
      NOW,
    );
    // Only the parent contributes; the count:0 entry adds nothing and never crashes.
    expect(counts).toEqual<FleetCount[]>([
      { role: "orchestrator", model: "opus", count: 1 },
    ]);
  });

  it("sums subagent counts of the same (role, model) across sessions", () => {
    const counts = fleetCounts(
      [
        session({
          agentId: "orch-1",
          role: "orchestrator",
          model: "opus",
          subagents: [{ label: "c", model: "sonnet", count: 2, role: "coder" }],
        }),
        session({
          agentId: "orch-2",
          role: "orchestrator",
          model: "opus",
          subagents: [{ label: "c", model: "sonnet", count: 4, role: "coder" }],
        }),
      ],
      NOW,
    );
    const coderSonnet = counts.find(
      (c) => c.role === "coder" && c.model === "sonnet",
    );
    expect(coderSonnet?.count).toBe(6);
    const orchOpus = counts.find(
      (c) => c.role === "orchestrator" && c.model === "opus",
    );
    expect(orchOpus?.count).toBe(2);
  });

  it("orders by role (orchestrator>coder>verifier>others) then model (opus>sonnet>haiku>others)", () => {
    const counts = fleetCounts(
      [
        session({ agentId: "z", role: "zebra", model: "opus" }),
        session({ agentId: "v", role: "verifier", model: "haiku" }),
        session({ agentId: "c2", role: "coder", model: "sonnet" }),
        session({ agentId: "c1", role: "coder", model: "opus" }),
        session({ agentId: "o", role: "orchestrator", model: "haiku" }),
        session({ agentId: "c3", role: "coder", model: "zylon" }),
      ],
      NOW,
    );
    expect(counts.map((c) => `${c.role}/${c.model}`)).toEqual([
      "orchestrator/haiku",
      "coder/opus",
      "coder/sonnet",
      "coder/zylon",
      "verifier/haiku",
      "zebra/opus",
    ]);
  });
});

describe("summarizeFleet", () => {
  it("renders 'count model role(s)' joined by ' · ' with naive pluralization", () => {
    const counts: FleetCount[] = [
      { role: "orchestrator", model: "opus", count: 1 },
      { role: "coder", model: "opus", count: 2 },
      { role: "reviewer", model: "sonnet", count: 6 },
    ];
    expect(summarizeFleet(counts)).toBe(
      "1 opus orchestrator · 2 opus coders · 6 sonnet reviewers",
    );
  });

  it("returns an empty string for an empty fleet", () => {
    expect(summarizeFleet([])).toBe("");
  });
});

describe("reportedSubagentTotal / fleetHeadlineSuffix", () => {
  it("sums live subagent counts and excludes closed sessions + count-0 entries", () => {
    const sessions = [
      session({
        agentId: "o",
        role: "orchestrator",
        subagents: [
          { label: "c", model: "sonnet", count: 3, role: "coder" },
          { label: "z", model: "haiku", count: 0, role: "coder" }, // dropped
        ],
      }),
      session({
        agentId: "closed",
        status: "done",
        subagents: [{ label: "x", model: "opus", count: 5, role: "coder" }], // excluded
      }),
      session({ agentId: "solo" }), // no subagents
    ];
    expect(reportedSubagentTotal(sessions, NOW)).toBe(3);
    expect(fleetHeadlineSuffix(sessions, NOW)).toBe(" (incl. reported subagents)");
  });

  it("returns 0 / empty suffix when no live subagents are reported", () => {
    const sessions = [
      session({ agentId: "a" }),
      session({
        agentId: "closed",
        status: "closed",
        subagents: [{ label: "x", model: "opus", count: 2, role: "coder" }],
      }),
    ];
    expect(reportedSubagentTotal(sessions, NOW)).toBe(0);
    expect(fleetHeadlineSuffix(sessions, NOW)).toBe("");
  });
});

describe("attentionSessions", () => {
  it("picks needsUser and stale/lost, drops healthy and closed", () => {
    const needs = session({
      agentId: "needs",
      lastSeenAt: FRESH,
      needsUser: {
        reason: "permission",
        message: "Allow write?",
        since: FRESH,
      },
    });
    const stale = session({ agentId: "stale", lastSeenAt: STALE });
    const lost = session({ agentId: "lost", lastSeenAt: LOST });
    const healthy = session({ agentId: "healthy", lastSeenAt: FRESH });
    const closed = session({
      agentId: "closed",
      status: "done",
      lastSeenAt: LOST,
    });

    const result = attentionSessions(
      [needs, stale, lost, healthy, closed],
      NOW,
    );
    expect(result.map((s) => s.agentId)).toEqual(["needs", "stale", "lost"]);
  });

  it("excludes a closed (done) session even if it has a leftover needsUser", () => {
    // A done session with a stale needsUser must NOT ring the bell: the close
    // filter runs BEFORE the needsUser check.
    const doneWithNeeds = session({
      agentId: "done-needs",
      status: "done",
      lastSeenAt: FRESH,
      needsUser: { reason: "question", message: "Stale?", since: FRESH },
    });
    const closedLiteral = session({
      agentId: "closed-needs",
      status: "closed",
      lastSeenAt: FRESH,
      needsUser: { reason: "permission", message: "Stale?", since: FRESH },
    });
    expect(attentionSessions([doneWithNeeds, closedLiteral], NOW)).toEqual([]);
  });

  it("treats a fresh-heartbeat needsUser session as attention even though it is online", () => {
    const s = session({
      agentId: "blocked",
      lastSeenAt: FRESH,
      needsUser: { reason: "question", message: "Which?", since: FRESH },
    });
    expect(attentionSessions([s], NOW).map((x) => x.agentId)).toEqual([
      "blocked",
    ]);
  });
});
