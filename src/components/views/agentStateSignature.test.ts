import { describe, expect, it } from "vitest";
import type { AgentLiveState } from "../../types/backend";
import { agentStateSignature } from "./ProjectsView";

// WARNING (frontend audit): get_agent_live_state can return null/undefined. The
// board poll must never call agentStateSignature(null) → TypeError on
// `.updatedAt`. These pin the null-safe contract.

function liveState(partial: Partial<AgentLiveState> = {}): AgentLiveState {
  return {
    version: 2,
    updatedAt: "2026-06-06T10:00:00.000Z",
    sessions: [],
    claims: [],
    events: [],
    rules: [],
    statePath: "",
    mcpCommand: "",
    mcpClientConfig: "",
    ...partial,
  };
}

describe("agentStateSignature null-safety", () => {
  it("returns a stable sentinel for null without throwing", () => {
    expect(() => agentStateSignature(null)).not.toThrow();
    expect(agentStateSignature(null)).toBe("∅");
  });

  it("returns the same sentinel for undefined", () => {
    expect(agentStateSignature(undefined)).toBe("∅");
  });

  it("a real state never collides with the null sentinel", () => {
    expect(agentStateSignature(liveState())).not.toBe("∅");
  });

  it("combines updatedAt with collection sizes", () => {
    const sig = agentStateSignature(
      liveState({
        updatedAt: "T",
        sessions: [{} as never],
        claims: [{} as never, {} as never],
        events: [],
      }),
    );
    expect(sig).toBe("T|1|2|0");
  });
});
