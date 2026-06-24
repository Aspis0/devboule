import { describe, it, expect } from "vitest";
import { agentChannel } from "./agentChannel";
import type { WorkNode } from "./workConsoleModel";

const mk = (over: Partial<WorkNode> = {}): WorkNode => ({
  agentId: "c1", type: "coder", file: "src/auth/login.ts", district: "auth",
  status: "running", label: "coder · codex", parentAgentId: null, taskId: "t1",
  pendingQuestion: null, live: true, children: [], orphaned: false, subagents: [], ...over,
});

// miniManaged = local mini OR local agentic coder (driven by mini_coder directives).
// !miniManaged = a cloud (claude/codex) worker running in a PTY.

describe("agentChannel — Direction A (message)", () => {
  it("routes a mini-managed agent to mini_coder_steer", () => {
    const ch = agentChannel(mk({ type: "coder" }), { miniManaged: true }, "message");
    expect(ch?.command).toBe("mini_coder_steer");
    expect(ch?.buildArgs("go")).toEqual({ agentId: "c1", message: "go" });
  });

  it("routes a cloud PTY worker to agent_pty_send_message", () => {
    const ch = agentChannel(mk({ type: "coder" }), { miniManaged: false }, "message");
    expect(ch?.command).toBe("agent_pty_send_message");
    expect(ch?.buildArgs("go")).toEqual({ agentId: "c1", message: "go" });
  });

  it("a mini node is ALWAYS mini-managed even if mistakenly marked cloud", () => {
    // A mini spawns with a real app PTY, but must be steered via the directive queue —
    // never raw-written to its PTY. The node type wins over the ctx flag.
    const ch = agentChannel(mk({ type: "mini", agentId: "m1" }), { miniManaged: false }, "message");
    expect(ch?.command).toBe("mini_coder_steer");
    expect(ch?.buildArgs("x")).toEqual({ agentId: "m1", message: "x" });
  });
});

describe("agentChannel — Direction B (answer)", () => {
  it("routes a cloud coder's answer to reply_to_agent", () => {
    const ch = agentChannel(mk({ type: "coder" }), { miniManaged: false }, "answer");
    expect(ch?.command).toBe("reply_to_agent");
    expect(ch?.buildArgs("use Auth0")).toEqual({ agentId: "c1", replyText: "use Auth0" });
  });

  it("routes a mini-managed agent's answer back through mini_coder_steer", () => {
    const ch = agentChannel(mk({ type: "coder" }), { miniManaged: true }, "answer");
    expect(ch?.command).toBe("mini_coder_steer");
    expect(ch?.buildArgs("use Auth0")).toEqual({ agentId: "c1", message: "use Auth0" });
  });

  it("routes a mini's answer through mini_coder_steer regardless of the ctx flag", () => {
    const ch = agentChannel(mk({ type: "mini", agentId: "m1" }), { miniManaged: false }, "answer");
    expect(ch?.command).toBe("mini_coder_steer");
  });
});

describe("agentChannel — not handled here", () => {
  it("returns null for the orchestrator (it is routed to the reused planner console)", () => {
    expect(agentChannel(mk({ type: "orchestrator", agentId: "o1" }), { miniManaged: true }, "message")).toBeNull();
    expect(agentChannel(mk({ type: "orchestrator", agentId: "o1" }), { miniManaged: false }, "answer")).toBeNull();
  });

  it("returns null for the censor (an automated reviewer, never messaged)", () => {
    expect(agentChannel(mk({ type: "censor", agentId: "z1" }), { miniManaged: true }, "message")).toBeNull();
    expect(agentChannel(mk({ type: "censor", agentId: "z1" }), { miniManaged: false }, "answer")).toBeNull();
  });
});
