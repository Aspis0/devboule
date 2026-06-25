import { describe, it, expect, beforeEach } from "vitest";
import {
  useWorkSelectionStore,
  taskIdForAgent,
  primaryAgentForTask,
} from "./workSelectionStore";
import type { AgentClaim, AgentSession } from "../types/backend";

// The work-selection store is the single shared selection bridging the Work Console
// (LivingPlan / FocusStage, owned by ProjectWorkspace) and the bottom DAG board
// (TaskCard, owned by ProjectsView). It holds BOTH the selected agent and the
// selected task so the two surfaces stay twinned. The agent<->task mapping is NOT
// in the store (it needs live session/claim data); two pure helpers resolve it,
// reusing deriveWorkers so the board's badge derivation and the selection agree.

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

function claim(overrides: Partial<AgentClaim>): AgentClaim {
  return {
    projectId: "p-1",
    projectTitle: null,
    taskId: "t-1",
    taskTitle: null,
    agentId: "a-1",
    role: "coder",
    status: "claimed",
    claimedAt: "2026-06-24T10:00:00.000Z",
    updatedAt: "2026-06-24T10:00:00.000Z",
    leaseUntil: null,
    evidence: null,
    ...overrides,
  };
}

describe("useWorkSelectionStore", () => {
  beforeEach(() => {
    useWorkSelectionStore.setState({ selectedAgentId: null, selectedTaskId: null });
  });

  it("selectBoth sets agent + task in a single atomic store write", () => {
    let snapshots = 0;
    let lastAgent: string | null = "sentinel";
    let lastTask: string | null = "sentinel";
    const unsub = useWorkSelectionStore.subscribe((s) => {
      snapshots += 1;
      lastAgent = s.selectedAgentId;
      lastTask = s.selectedTaskId;
    });
    useWorkSelectionStore.getState().selectBoth("a-9", "t-9");
    unsub();
    // Exactly one subscriber notification (one set), with BOTH fields already final —
    // no half-updated (agent set, task stale) intermediate snapshot.
    expect(snapshots).toBe(1);
    expect(lastAgent).toBe("a-9");
    expect(lastTask).toBe("t-9");
  });

  it("clear resets both ids", () => {
    useWorkSelectionStore.setState({ selectedAgentId: "a-9", selectedTaskId: "t-9" });
    useWorkSelectionStore.getState().clear();
    expect(useWorkSelectionStore.getState().selectedAgentId).toBeNull();
    expect(useWorkSelectionStore.getState().selectedTaskId).toBeNull();
  });
});

describe("taskIdForAgent", () => {
  it("resolves via the session's currentTaskId first", () => {
    const sessions = [session({ agentId: "a-1", currentTaskId: "t-from-session" })];
    const claims = [claim({ agentId: "a-1", taskId: "t-from-claim" })];
    expect(taskIdForAgent("a-1", sessions, claims)).toBe("t-from-session");
  });

  it("falls back to the claim's taskId when the session has no currentTaskId", () => {
    const sessions = [session({ agentId: "a-1", currentTaskId: null })];
    const claims = [claim({ agentId: "a-1", taskId: "t-from-claim" })];
    expect(taskIdForAgent("a-1", sessions, claims)).toBe("t-from-claim");
  });

  it("returns null for a null agent or an agent with no task anywhere", () => {
    expect(taskIdForAgent(null, [], [])).toBeNull();
    const sessions = [session({ agentId: "a-1", currentTaskId: null })];
    expect(taskIdForAgent("a-1", sessions, [])).toBeNull();
  });

  it("rejects falsy/empty/undefined agent ids (not just null)", () => {
    const sessions = [session({ agentId: "", currentTaskId: "t-x" })];
    expect(taskIdForAgent("", sessions, [])).toBeNull();
    // A JSON-deserialized session can have an absent (undefined) currentTaskId.
    const absent = [session({ agentId: "a-1" })];
    (absent[0] as { currentTaskId?: string | null }).currentTaskId = undefined;
    expect(taskIdForAgent("a-1", absent, [])).toBeNull();
  });

  it("the LIVE session wins over a claim on a different task (documents the direction)", () => {
    // Agent holds a claim on t-1 but its session is now live on t-2: the agent's
    // task follows the session, so the console/LivingPlan and this helper agree.
    const sessions = [session({ agentId: "a-1", currentTaskId: "t-2" })];
    const claims = [claim({ agentId: "a-1", taskId: "t-1" })];
    expect(taskIdForAgent("a-1", sessions, claims)).toBe("t-2");
  });
});

describe("primaryAgentForTask", () => {
  it("picks the first worker of the task (claims before sessions, deriveWorkers order)", () => {
    const claims = [
      claim({ agentId: "claimer", taskId: "t-1" }),
      claim({ agentId: "other", taskId: "t-2" }),
    ];
    const sessions = [session({ agentId: "sessioner", currentTaskId: "t-1" })];
    expect(primaryAgentForTask("t-1", claims, sessions)).toBe("claimer");
  });

  it("uses a session worker when the task has no claim", () => {
    const sessions = [session({ agentId: "sessioner", currentTaskId: "t-1" })];
    expect(primaryAgentForTask("t-1", [], sessions)).toBe("sessioner");
  });

  it("returns null for a null task or a task with no workers", () => {
    expect(primaryAgentForTask(null, [], [])).toBeNull();
    expect(primaryAgentForTask("t-1", [], [])).toBeNull();
  });

  it("breaks a multi-claim tie deterministically (first claim, matching board badge order)", () => {
    // Two agents claim the same task: deriveWorkers keeps claims-first arrival order, so the
    // primary == the first badge the board renders. Twinning highlights the SAME agent the user sees.
    const claims = [
      claim({ agentId: "first", taskId: "t-1" }),
      claim({ agentId: "second", taskId: "t-1" }),
    ];
    expect(primaryAgentForTask("t-1", claims, [])).toBe("first");
  });
});
