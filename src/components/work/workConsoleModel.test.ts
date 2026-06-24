import { describe, it, expect } from "vitest";
import { buildWorkConsoleModel, findWorkNode } from "./workConsoleModel";
import type { AgentSession, ProjectTask } from "../../types/backend";

const PROJECT = "proj-1";

function session(p: Partial<AgentSession> & { agentId: string }): AgentSession {
  return {
    agentId: p.agentId,
    role: p.role ?? "coder",
    model: p.model ?? null,
    // preserve an explicitly-injected null (IPC fiction test); only default when absent
    status: "status" in p ? (p.status as string) : "running",
    message: p.message ?? null,
    client: p.client ?? null,
    currentProjectId: p.currentProjectId ?? PROJECT,
    currentTaskId: p.currentTaskId ?? null,
    firstSeenAt: null,
    lastSeenAt: null,
    parentAgentId: p.parentAgentId ?? null,
    pendingQuestion: p.pendingQuestion ?? null,
    host: p.host ?? "app",
    subagents: p.subagents,
  };
}

function task(p: Partial<ProjectTask> & { id: string }): ProjectTask {
  return {
    id: p.id,
    title: p.title ?? p.id,
    status: p.status ?? "wip",
    priority: null,
    assignee: null,
    due: null,
    linkedResources: [],
    updatedAt: "2026-06-24T00:00:00Z",
    suspectFileIds: [],
    scope: p.scope,
    dependsOn: p.dependsOn,
    planId: p.planId,
  } as ProjectTask;
}

describe("buildWorkConsoleModel", () => {
  it("groups top-level coders into districts derived from their file path", () => {
    const tasks = [
      task({ id: "t-board", scope: ["src/views/projects/board.tsx"] }),
      task({ id: "t-login", scope: ["src/auth/login.ts"] }),
    ];
    const sessions = [
      session({ agentId: "c1", currentTaskId: "t-board" }),
      session({ agentId: "c2", currentTaskId: "t-login" }),
    ];
    const m = buildWorkConsoleModel({ sessions, tasks, projectId: PROJECT });

    const names = m.districts.map((d) => d.name).sort();
    expect(names).toEqual(["auth", "projects"]);

    const projects = m.districts.find((d) => d.name === "projects")!;
    expect(projects.nodes).toHaveLength(1);
    expect(projects.nodes[0].agentId).toBe("c1");
    expect(projects.nodes[0].file).toBe("src/views/projects/board.tsx");
    expect(projects.nodes[0].type).toBe("coder");
  });

  it("nests a mini (parentAgentId set) under its parent coder, not as a top-level node", () => {
    const tasks = [
      task({ id: "t-board", scope: ["src/views/projects/board.tsx"] }),
      task({ id: "t-card", scope: ["src/views/projects/card.tsx"] }),
    ];
    const sessions = [
      session({ agentId: "c1", currentTaskId: "t-board" }),
      session({ agentId: "m1", currentTaskId: "t-card", parentAgentId: "c1", model: "qwen-32b" }),
    ];
    const m = buildWorkConsoleModel({ sessions, tasks, projectId: PROJECT });

    const projects = m.districts.find((d) => d.name === "projects")!;
    // only the coder is a top-level node; the mini is its child
    expect(projects.nodes).toHaveLength(1);
    const coder = projects.nodes[0];
    expect(coder.agentId).toBe("c1");
    expect(coder.children).toHaveLength(1);
    expect(coder.children[0].agentId).toBe("m1");
    expect(coder.children[0].type).toBe("mini");
    expect(coder.children[0].parentAgentId).toBe("c1");
  });

  it("puts an agent with no task/scope into unplaced", () => {
    const sessions = [session({ agentId: "c1", currentTaskId: null })];
    const m = buildWorkConsoleModel({ sessions, tasks: [], projectId: PROJECT });
    expect(m.districts).toHaveLength(0);
    expect(m.unplaced.map((n) => n.agentId)).toEqual(["c1"]);
    expect(m.unplaced[0].file).toBeNull();
  });

  it("places the orchestrator at the civic root, never inside a district", () => {
    const sessions = [
      session({ agentId: "o1", client: "orchestrator", role: "coder", currentTaskId: null }),
      session({ agentId: "c1", currentTaskId: "t1" }),
    ];
    const tasks = [task({ id: "t1", scope: ["src/auth/login.ts"] })];
    const m = buildWorkConsoleModel({ sessions, tasks, projectId: PROJECT });

    expect(m.orchestrator?.agentId).toBe("o1");
    expect(m.orchestrator?.type).toBe("orchestrator");
    const allDistrictIds = m.districts.flatMap((d) => d.nodes.map((n) => n.agentId));
    expect(allDistrictIds).not.toContain("o1");
    expect(m.unplaced.map((n) => n.agentId)).not.toContain("o1");
  });

  it("joins node↔task: carries taskId and file from task.scope[0]", () => {
    const tasks = [task({ id: "t1", scope: ["src/auth/login.ts", "src/auth/session.ts"] })];
    const sessions = [session({ agentId: "c1", currentTaskId: "t1" })];
    const m = buildWorkConsoleModel({ sessions, tasks, projectId: PROJECT });
    const node = m.districts[0].nodes[0];
    expect(node.taskId).toBe("t1");
    expect(node.file).toBe("src/auth/login.ts");
  });

  it("carries a pending question and derives live from status", () => {
    const tasks = [task({ id: "t1", scope: ["src/auth/login.ts"] })];
    const sessions = [
      session({
        agentId: "c1",
        currentTaskId: "t1",
        status: "running",
        pendingQuestion: { id: "q1", question: "which auth provider?", createdAt: "2026-06-24T00:00:00Z" },
      }),
      session({ agentId: "c2", currentTaskId: "t1", status: "idle" }),
    ];
    const m = buildWorkConsoleModel({ sessions, tasks, projectId: PROJECT });
    const nodes = m.districts[0].nodes;
    const c1 = nodes.find((n) => n.agentId === "c1")!;
    const c2 = nodes.find((n) => n.agentId === "c2")!;
    expect(c1.pendingQuestion).toBe("which auth provider?");
    expect(c1.live).toBe(true);
    expect(c2.pendingQuestion).toBeNull();
    expect(c2.live).toBe(false);
  });

  it("ignores sessions from other projects", () => {
    const tasks = [task({ id: "t1", scope: ["src/auth/login.ts"] })];
    const sessions = [
      session({ agentId: "c1", currentTaskId: "t1", currentProjectId: "other" }),
    ];
    const m = buildWorkConsoleModel({ sessions, tasks, projectId: PROJECT });
    expect(m.districts).toHaveLength(0);
    expect(m.unplaced).toHaveLength(0);
  });

  it("does not silently drop a mini whose parent is absent", () => {
    const tasks = [task({ id: "t-card", scope: ["src/views/projects/card.tsx"] })];
    const sessions = [
      session({ agentId: "m1", currentTaskId: "t-card", parentAgentId: "ghost" }),
    ];
    const m = buildWorkConsoleModel({ sessions, tasks, projectId: PROJECT });
    const allIds = [
      ...m.districts.flatMap((d) => d.nodes.flatMap((n) => [n.agentId, ...n.children.map((c) => c.agentId)])),
      ...m.unplaced.map((n) => n.agentId),
    ];
    expect(allIds).toContain("m1");
  });
});

// Helper: every agentId reachable anywhere in the model (orchestrator + its children,
// district nodes + their children recursively, unplaced).
function flattenIds(m: ReturnType<typeof buildWorkConsoleModel>): string[] {
  const out: string[] = [];
  const walk = (n: { agentId: string; children: { agentId: string; children: unknown[] }[] }) => {
    out.push(n.agentId);
    n.children.forEach((c) => walk(c as never));
  };
  if (m.orchestrator) walk(m.orchestrator as never);
  m.districts.forEach((d) => d.nodes.forEach((n) => walk(n as never)));
  m.unplaced.forEach((n) => walk(n as never));
  return out;
}

describe("buildWorkConsoleModel — hardening (reviewer findings)", () => {
  it("does not crash and is not-live when status is null/missing (IPC fiction)", () => {
    const tasks = [task({ id: "t1", scope: ["src/auth/login.ts"] })];
    const sessions = [
      // status can arrive null over Tauri IPC despite the non-null TS type.
      session({ agentId: "c1", currentTaskId: "t1", status: null as unknown as string }),
    ];
    const m = buildWorkConsoleModel({ sessions, tasks, projectId: PROJECT });
    const node = m.districts[0].nodes[0];
    expect(node.agentId).toBe("c1");
    expect(node.live).toBe(false);
  });

  it("treats role==='orchestrator' as the orchestrator even with no client stamp", () => {
    const sessions = [
      session({ agentId: "o1", role: "orchestrator", client: null, currentTaskId: null }),
    ];
    const m = buildWorkConsoleModel({ sessions, tasks: [], projectId: PROJECT });
    expect(m.orchestrator?.agentId).toBe("o1");
    expect(m.orchestrator?.type).toBe("orchestrator");
  });

  it("nests a mini whose parent IS the orchestrator under the orchestrator", () => {
    const tasks = [task({ id: "t1", scope: ["src/views/projects/card.tsx"] })];
    const sessions = [
      session({ agentId: "o1", client: "orchestrator", currentTaskId: null }),
      session({ agentId: "m1", parentAgentId: "o1", currentTaskId: "t1" }),
    ];
    const m = buildWorkConsoleModel({ sessions, tasks, projectId: PROJECT });
    expect(m.orchestrator?.children.map((c) => c.agentId)).toEqual(["m1"]);
    const districtIds = m.districts.flatMap((d) => d.nodes.map((n) => n.agentId));
    expect(districtIds).not.toContain("m1");
    expect(m.unplaced.map((n) => n.agentId)).not.toContain("m1");
  });

  it("normalizes a leading-slash absolute path into a real district (never empty name)", () => {
    const tasks = [task({ id: "t1", scope: ["/src/auth/login.ts"] })];
    const sessions = [session({ agentId: "c1", currentTaskId: "t1" })];
    const m = buildWorkConsoleModel({ sessions, tasks, projectId: PROJECT });
    expect(m.districts.map((d) => d.name)).toEqual(["auth"]);
    expect(m.districts.every((d) => d.name.length > 0)).toBe(true);
  });

  it("classifies a censor as censor even when it carries a parentAgentId", () => {
    const tasks = [task({ id: "t1", scope: ["src-tauri/src/model.rs"] })];
    const sessions = [
      session({ agentId: "c1", currentTaskId: "t1" }),
      session({ agentId: "z1", role: "censor", parentAgentId: "c1", currentTaskId: "t1" }),
    ];
    const m = buildWorkConsoleModel({ sessions, tasks, projectId: PROJECT });
    const all = [
      ...m.districts.flatMap((d) => d.nodes.flatMap((n) => [n, ...n.children])),
      ...m.unplaced,
    ];
    const censor = all.find((n) => n.agentId === "z1");
    expect(censor?.type).toBe("censor");
  });

  it("does not lose a second orchestrator session", () => {
    const tasks = [task({ id: "t1", scope: ["src/auth/login.ts"] })];
    const sessions = [
      session({ agentId: "o1", client: "orchestrator", currentTaskId: null }),
      session({ agentId: "o2", client: "orchestrator", currentTaskId: "t1" }),
    ];
    const m = buildWorkConsoleModel({ sessions, tasks, projectId: PROJECT });
    expect(m.orchestrator?.agentId).toBe("o1");
    // the second orchestrator must still be reachable somewhere, not silently dropped
    expect(flattenIds(m)).toContain("o2");
  });

  it("supports a nested mini chain (mini under mini under coder)", () => {
    const tasks = [
      task({ id: "t-c", scope: ["src/views/projects/board.tsx"] }),
      task({ id: "t-m1", scope: ["src/views/projects/card.tsx"] }),
      task({ id: "t-m2", scope: ["src/views/projects/row.tsx"] }),
    ];
    const sessions = [
      session({ agentId: "c1", currentTaskId: "t-c" }),
      session({ agentId: "m1", parentAgentId: "c1", currentTaskId: "t-m1" }),
      session({ agentId: "m2", parentAgentId: "m1", currentTaskId: "t-m2" }),
    ];
    const m = buildWorkConsoleModel({ sessions, tasks, projectId: PROJECT });
    const coder = m.districts.flatMap((d) => d.nodes).find((n) => n.agentId === "c1")!;
    const m1 = coder.children.find((c) => c.agentId === "m1")!;
    expect(m1).toBeTruthy();
    expect(m1.children.map((c) => c.agentId)).toEqual(["m2"]);
  });

  it("carries heartbeat-reported subagents onto the node (parity with the old rail)", () => {
    const tasks = [task({ id: "t1", scope: ["src/auth/login.ts"] })];
    const sessions = [
      session({
        agentId: "c1",
        currentTaskId: "t1",
        subagents: [
          { label: "writer", model: "sonnet", count: 2 },
          { label: "tester", model: "haiku", count: 1 },
        ],
      }),
    ];
    const m = buildWorkConsoleModel({ sessions, tasks, projectId: PROJECT });
    const node = m.districts[0].nodes[0];
    expect(node.subagents).toEqual([
      { label: "writer", count: 2 },
      { label: "tester", count: 1 },
    ]);
  });

  it("flags an orphaned mini (parent absent) and not a healthy one", () => {
    const tasks = [
      task({ id: "t-c", scope: ["src/views/projects/board.tsx"] }),
      task({ id: "t-m", scope: ["src/views/projects/card.tsx"] }),
      task({ id: "t-o", scope: ["src/auth/login.ts"] }),
    ];
    const sessions = [
      session({ agentId: "c1", currentTaskId: "t-c" }),
      session({ agentId: "m-ok", parentAgentId: "c1", currentTaskId: "t-m" }),
      session({ agentId: "m-orphan", parentAgentId: "ghost", currentTaskId: "t-o" }),
    ];
    const m = buildWorkConsoleModel({ sessions, tasks, projectId: PROJECT });
    const all = [
      ...m.districts.flatMap((d) => d.nodes.flatMap((n) => [n, ...n.children])),
      ...m.unplaced,
    ];
    expect(all.find((n) => n.agentId === "m-ok")?.orphaned).toBe(false);
    expect(all.find((n) => n.agentId === "m-orphan")?.orphaned).toBe(true);
  });

  it("does not throw on duplicate agentIds", () => {
    const tasks = [task({ id: "t1", scope: ["src/auth/login.ts"] })];
    const sessions = [
      session({ agentId: "dup", currentTaskId: "t1" }),
      session({ agentId: "dup", currentTaskId: "t1" }),
    ];
    expect(() => buildWorkConsoleModel({ sessions, tasks, projectId: PROJECT })).not.toThrow();
  });
});

describe("findWorkNode", () => {
  const model = () =>
    buildWorkConsoleModel({
      projectId: PROJECT,
      tasks: [
        task({ id: "t-board", scope: ["src/views/projects/board.tsx"] }),
        task({ id: "t-card", scope: ["src/views/projects/card.tsx"] }),
      ],
      sessions: [
        session({ agentId: "o1", client: "orchestrator", currentTaskId: null }),
        session({ agentId: "c1", currentTaskId: "t-board" }),
        session({ agentId: "m1", currentTaskId: "t-card", parentAgentId: "c1" }),
      ],
    });

  it("finds the orchestrator, a district coder, and a nested mini", () => {
    expect(findWorkNode(model(), "o1")?.type).toBe("orchestrator");
    expect(findWorkNode(model(), "c1")?.agentId).toBe("c1");
    expect(findWorkNode(model(), "m1")?.type).toBe("mini");
  });

  it("returns null for an unknown agentId", () => {
    expect(findWorkNode(model(), "nope")).toBeNull();
  });
});
