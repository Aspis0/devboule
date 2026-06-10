import { describe, it, expect, beforeEach } from "vitest";
import { useAgentAttentionStore } from "./agentAttentionStore";
import type { AgentLiveState, AgentSession } from "../types/backend";

// The attention store is a passive sink: it holds the latest sessions pushed by
// the existing live-state pollers. These tests pin setFromLiveState's behavior.

function session(overrides: Partial<AgentSession>): AgentSession {
  return {
    agentId: "a-1",
    role: "coder",
    model: "sonnet",
    status: "wip",
    message: null,
    currentProjectId: null,
    currentTaskId: null,
    firstSeenAt: null,
    lastSeenAt: null,
    ...overrides,
  };
}

function liveState(sessions: AgentSession[]): AgentLiveState {
  return {
    version: 2,
    updatedAt: "2026-06-04T10:00:00.000Z",
    sessions,
    claims: [],
    events: [],
    rules: [],
    statePath: "",
    mcpCommand: "",
    mcpClientConfig: "",
  };
}

describe("agentAttentionStore.setFromLiveState", () => {
  beforeEach(() => {
    useAgentAttentionStore.setState({ sessions: [], updatedAt: 0 });
  });

  it("clears to an empty fleet on a null state", () => {
    useAgentAttentionStore.getState().setFromLiveState(liveState([session({})]));
    expect(useAgentAttentionStore.getState().sessions).toHaveLength(1);
    useAgentAttentionStore.getState().setFromLiveState(null);
    expect(useAgentAttentionStore.getState().sessions).toEqual([]);
  });

  it("stores the sessions from the live state", () => {
    const sessions = [session({ agentId: "x" }), session({ agentId: "y" })];
    useAgentAttentionStore.getState().setFromLiveState(liveState(sessions));
    expect(useAgentAttentionStore.getState().sessions.map((s) => s.agentId)).toEqual(["x", "y"]);
  });

  it("bumps updatedAt on every accepted (changed) update", () => {
    // Empty -> one session is a real change, so updatedAt advances past 0.
    useAgentAttentionStore.getState().setFromLiveState(liveState([session({})]));
    const first = useAgentAttentionStore.getState().updatedAt;
    expect(first).toBeGreaterThan(0);
    // One -> two sessions is another real change, so updatedAt does not regress.
    useAgentAttentionStore
      .getState()
      .setFromLiveState(liveState([session({ agentId: "a" }), session({ agentId: "b" })]));
    const second = useAgentAttentionStore.getState().updatedAt;
    expect(second).toBeGreaterThanOrEqual(first);
  });

  it("keeps the SAME sessions reference when the snapshot is unchanged (#3)", () => {
    const sessions = [session({ agentId: "x" }), session({ agentId: "y" })];
    useAgentAttentionStore.getState().setFromLiveState(liveState(sessions));
    const firstRef = useAgentAttentionStore.getState().sessions;
    const firstUpdatedAt = useAgentAttentionStore.getState().updatedAt;
    // A structurally identical state (fresh objects, same ids + needsUser-since)
    // must NOT change the stored reference, so the Header does not re-render and
    // the watcher does not re-fire on a no-op poll tick.
    useAgentAttentionStore
      .getState()
      .setFromLiveState(
        liveState([session({ agentId: "x" }), session({ agentId: "y" })]),
      );
    expect(useAgentAttentionStore.getState().sessions).toBe(firstRef);
    expect(useAgentAttentionStore.getState().updatedAt).toBe(firstUpdatedAt);
  });

  it("does change the reference when a needsUser.since transitions (#3)", () => {
    useAgentAttentionStore
      .getState()
      .setFromLiveState(liveState([session({ agentId: "x" })]));
    const firstRef = useAgentAttentionStore.getState().sessions;
    useAgentAttentionStore.getState().setFromLiveState(
      liveState([
        session({
          agentId: "x",
          needsUser: {
            reason: "needs_user",
            message: "help",
            since: "2026-06-04T10:01:00.000Z",
          },
        }),
      ]),
    );
    expect(useAgentAttentionStore.getState().sessions).not.toBe(firstRef);
  });
});
