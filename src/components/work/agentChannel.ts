import type { WorkNode } from "./workConsoleModel";

export type CommsDirection = "message" | "answer";
// miniManaged = the agent is driven by mini_coder directives (a local mini OR a local agentic
// coder) and MUST be steered via the directive queue. !miniManaged = a cloud (claude/codex)
// worker running in a raw PTY.
export interface ChannelCtx { miniManaged: boolean }
export interface AgentChannel {
  command: string;
  buildArgs: (text: string) => Record<string, unknown>;
}
export function agentChannel(node: WorkNode, ctx: ChannelCtx, dir: CommsDirection): AgentChannel | null {
  if (node.type === "orchestrator") return null;

  const isMiniManaged = node.type === "mini" || ctx.miniManaged;

  if (isMiniManaged) {
    return {
      command: "mini_coder_steer",
      buildArgs: (text: string) => ({ agentId: node.agentId, message: text }),
    };
  }

  if (dir === "message") {
    return {
      command: "agent_pty_send_message",
      buildArgs: (text: string) => ({ agentId: node.agentId, message: text }),
    };
  }

  if (dir === "answer") {
    return {
      command: "reply_to_agent",
      buildArgs: (text: string) => ({ agentId: node.agentId, replyText: text }),
    };
  }

  return null;
}
