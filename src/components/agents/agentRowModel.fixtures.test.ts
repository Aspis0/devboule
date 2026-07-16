// Fixture-based tests for the agent row model: prove the frontend models consume
// the REAL backend snapshot (rig/fixtures/agents-state.json) correctly.

import { describe, it, expect } from "vitest";
import {
  rowBadges,
  rowActions,
  fleetHealthRollup,
  drawerData,
} from "./agentRowModel";
import type {
  AgentSession,
  AgentClaim,
  AgentEvent,
  AgentLiveState,
} from "../../types/backend";
import fixture from "../../../rig/fixtures/agents-state.json";

const state = fixture as AgentLiveState;

describe("agentRowModel against agents-state fixture", () => {
  const sessions: AgentSession[] = state.sessions;
  const claims: AgentClaim[] = state.claims;
  const events: AgentEvent[] = state.events;

  // Fixture timestamps are all 2026-07-16T00:00:00Z. Use a `now` close to the
  // fixture's lastSeenAt so health is computed correctly (not years in the past).
  const NOW = Date.parse("2026-07-16T00:00:10.000Z"); // 10s after fixture time

  const orch = sessions.find((s) => s.agentId === "orch-1")!;
  const coder = sessions.find((s) => s.agentId === "coder-1")!;
  const mini = sessions.find((s) => s.agentId === "mini-1")!;

  it("has exactly 3 sessions", () => {
    expect(sessions).toHaveLength(3);
  });

  // ---- rowBadges ----

  it("rowBadges: orchestrator orch-1 is active, online, Claude client", () => {
    const badges = rowBadges(orch, NOW);
    expect(badges.health).toBe("online");
    expect(badges.modelLabel).toBe("claude-sonnet-4-20250514");
    expect(badges.modelKnown).toBe(true);
    expect(badges.cli.label).toBe("Claude");
  });

  it("rowBadges: coder-1 is active, online", () => {
    const badges = rowBadges(coder, NOW);
    expect(badges.health).toBe("online");
    expect(badges.modelLabel).toBe("claude-sonnet-4-20250514");
  });

  it("rowBadges: mini-1 is active, online, with parentAgentId", () => {
    const badges = rowBadges(mini, NOW);
    expect(badges.health).toBe("online");
    expect(badges.modelLabel).toBe("gemini-2.5-flash");
    expect(mini.parentAgentId).toBe("coder-1");
  });

  // ---- rowActions ----

  it("rowActions: app-hosted sessions show terminal toggle (when PTY), no open-CLI", () => {
    for (const s of sessions) {
      expect(s.host).toBe("app");
    }
    const actions = rowActions(orch, true);
    expect(actions.showTerminalToggle).toBe(true);
    expect(actions.showOpenCli).toBe(false);
    expect(actions.showExitedHint).toBe(false);
  });

  it("rowActions: app-hosted without PTY shows exited hint", () => {
    const actions = rowActions(orch, false);
    expect(actions.showTerminalToggle).toBe(false);
    expect(actions.showOpenCli).toBe(false);
    expect(actions.showExitedHint).toBe(true);
  });

  // ---- fleetHealthRollup ----

  it("fleetHealthRollup: all 3 sessions are online → online=3, stale=0, lost=0", () => {
    const rollup = fleetHealthRollup(sessions, NOW);
    expect(rollup).toEqual({ online: 3, stale: 0, lost: 0 });
  });

  // ---- drawerData ----

  it("drawerData: returns empty claims/events/subagents for a session with no claims/events", () => {
    const data = drawerData(orch, claims, events);
    expect(data.activeClaims).toHaveLength(0);
    expect(data.waitingClaims).toHaveLength(0);
    expect(data.historyClaims).toHaveLength(0);
    expect(data.events).toHaveLength(0);
    expect(data.subagents).toEqual([]);
  });

  // ---- stuckReport gap ----

  it.todo(
    "drawerData does NOT surface stuckReport from miniCoderDirectives — the model " +
    "does not read directives. stuckReport is surfaced via a SEPARATE " +
    "useMiniStuckReports hook + mini://stuck event channel. The drawer has no " +
    "visibility into directive-level stuck reports.",
  );

  it("miniCoderDirectives are present in the fixture but the model does not consume them", () => {
    const directives = state.miniCoderDirectives;
    expect(directives).toBeDefined();
    expect(Array.isArray(directives)).toBe(true);
    expect(directives!.length).toBe(3);
  });
});
