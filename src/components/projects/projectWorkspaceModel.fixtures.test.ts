// Fixture-based tests for the workspace model: prove the frontend models consume
// the REAL backend snapshot (rig/fixtures/agents-state.json) correctly.

import { describe, it, expect } from "vitest";
import {
  railRows,
  isMiniSession,
  isMiniManagedSession,
  reconcileSelectedAgentId,
} from "./projectWorkspaceModel";
import type { AgentSession, AgentLiveState } from "../../types/backend";
import fixture from "../../../rig/fixtures/agents-state.json";

const state = fixture as AgentLiveState;

describe("projectWorkspaceModel against agents-state fixture", () => {
  const sessions: AgentSession[] = state.sessions;

  // ---- isMiniSession ----

  it("isMiniSession: only the session with parentAgentId is a mini", () => {
    expect(isMiniSession(sessions[0])).toBe(false); // orch-1
    expect(isMiniSession(sessions[1])).toBe(false); // coder-1
    expect(isMiniSession(sessions[2])).toBe(true);  // mini-1
  });

  // ---- isMiniManagedSession ----

  it("isMiniManagedSession: mini is always managed; claude client is NOT managed at top-level", () => {
    // mini-1 is always mini-managed (isMiniSession => true)
    expect(isMiniManagedSession(sessions[2])).toBe(true);
    // orch-1 and coder-1 have client="claude" → NOT mini-managed (raw PTY worker)
    expect(isMiniManagedSession(sessions[0])).toBe(false);
    expect(isMiniManagedSession(sessions[1])).toBe(false);
  });

  // ---- railRows ----

  it("railRows: partitions sessions — 2 top-level, mini nests under coder-1", () => {
    const rows = railRows(sessions, null);
    expect(rows).toHaveLength(2);

    // First row: orch-1 (orchestrator)
    expect(rows[0].agentId).toBe("orch-1");
    expect(rows[0].role).toBe("orchestrator");
    expect(rows[0].orchestratorBadge).toBe(true);
    expect(rows[0].isMini).toBe(false);
    expect(rows[0].orphanedMini).toBe(false);
    expect(rows[0].miniChildren).toHaveLength(0);

    // Second row: coder-1 with mini-1 nested
    expect(rows[1].agentId).toBe("coder-1");
    expect(rows[1].role).toBe("coder");
    expect(rows[1].orchestratorBadge).toBe(false);
    expect(rows[1].isMini).toBe(false);
    expect(rows[1].miniChildren).toHaveLength(1);
    expect(rows[1].miniChildren[0].agentId).toBe("mini-1");
    expect(rows[1].miniChildren[0].isMini).toBe(true);
    expect(rows[1].miniChildren[0].role).toBe("coder"); // mini-coder role → coder
  });

  it("railRows: selection is threaded to rows and nested children", () => {
    const rows = railRows(sessions, "mini-1");
    expect(rows[0].selected).toBe(false);
    expect(rows[1].selected).toBe(false);
    expect(rows[1].miniChildren[0].selected).toBe(true);
  });

  it("railRows: selection on top-level coder", () => {
    const rows = railRows(sessions, "coder-1");
    expect(rows[0].selected).toBe(false);
    expect(rows[1].selected).toBe(true);
    expect(rows[1].miniChildren[0].selected).toBe(false);
  });

  // ---- reconcileSelectedAgentId ----

  it("reconcileSelectedAgentId: keeps existing valid selection unchanged", () => {
    const result = reconcileSelectedAgentId("orch-1", sessions);
    expect(result).toBe("orch-1");
  });

  it("reconcileSelectedAgentId: prunes a vanished id, falls back to freshest", () => {
    const result = reconcileSelectedAgentId("gone-agent", sessions);
    expect(result).not.toBeNull();
    expect(sessions.some((s) => s.agentId === result)).toBe(true);
  });

  it("reconcileSelectedAgentId: when a mini disappears, falls back to its parent (with previousSessions)", () => {
    const withoutMini = sessions.filter((s) => s.agentId !== "mini-1");
    const result = reconcileSelectedAgentId(
      "mini-1",
      withoutMini,
      sessions, // previousSessions — contains the now-gone mini-1
    );
    expect(result).toBe("coder-1"); // parent fallback
  });

  it("reconcileSelectedAgentId: without previousSessions, just falls back to freshest", () => {
    const withoutMini = sessions.filter((s) => s.agentId !== "mini-1");
    const result = reconcileSelectedAgentId("mini-1", withoutMini);
    expect(result).not.toBeNull();
    expect(withoutMini.some((s) => s.agentId === result)).toBe(true);
  });
});
