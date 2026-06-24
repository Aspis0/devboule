// @vitest-environment jsdom
//
// LivingPlan is the left "Living Plan" navigator: districts as frames, files as nodes,
// an agent inhabiting the node it edits, minis nested under their parent, the orchestrator
// at the civic root, live pulse / dirty coral / asks amber, and click-to-select.
// Pure render from a WorkConsoleModel + data-* hooks (used by tests, board twinning, Censor).

import { describe, it, expect, afterEach } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { LivingPlan } from "./LivingPlan";
import { buildWorkConsoleModel } from "./workConsoleModel";
import type { AgentSession, ProjectTask } from "../../types/backend";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const PROJECT = "p1";
const sess = (p: Partial<AgentSession> & { agentId: string }): AgentSession =>
  ({
    agentId: p.agentId, role: p.role ?? "coder", model: p.model ?? null,
    status: p.status ?? "running", message: null, client: p.client ?? null,
    currentProjectId: PROJECT, currentTaskId: p.currentTaskId ?? null,
    firstSeenAt: null, lastSeenAt: null, parentAgentId: p.parentAgentId ?? null,
    pendingQuestion: p.pendingQuestion ?? null, host: "app",
  }) as AgentSession;
const tsk = (id: string, scope: string[]): ProjectTask =>
  ({ id, title: id, status: "wip", priority: null, assignee: null, due: null,
     linkedResources: [], updatedAt: "x", suspectFileIds: [], scope }) as ProjectTask;

const baseModel = () =>
  buildWorkConsoleModel({
    projectId: PROJECT,
    tasks: [
      tsk("t-board", ["src/views/projects/board.tsx"]),
      tsk("t-card", ["src/views/projects/card.tsx"]),
      tsk("t-login", ["src/auth/login.ts"]),
    ],
    sessions: [
      sess({ agentId: "o1", client: "orchestrator", currentTaskId: null }),
      sess({ agentId: "c1", currentTaskId: "t-board", client: "codex" }),
      sess({ agentId: "m1", currentTaskId: "t-card", parentAgentId: "c1", model: "qwen-32b" }),
      sess({ agentId: "c2", currentTaskId: "t-login", status: "idle" }),
    ],
  });

const html = (props: Parameters<typeof LivingPlan>[0]) =>
  renderToStaticMarkup(createElement(LivingPlan, props));

let root: Root | null = null;
let container: HTMLDivElement | null = null;
afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  if (container) container.remove();
  container = null;
});

describe("LivingPlan", () => {
  it("renders district frames for the active districts", () => {
    const out = html({ model: baseModel(), selectedAgentId: null, onSelect: () => {} });
    expect(out).toContain('data-district="projects"');
    expect(out).toContain('data-district="auth"');
  });

  it("renders the orchestrator at the civic root", () => {
    const out = html({ model: baseModel(), selectedAgentId: null, onSelect: () => {} });
    expect(out).toContain('data-agent-id="o1"');
    expect(out).toContain('data-node-type="orchestrator"');
  });

  it("renders a coder node inhabiting its file", () => {
    const out = html({ model: baseModel(), selectedAgentId: null, onSelect: () => {} });
    expect(out).toContain('data-agent-id="c1"');
    expect(out).toContain('data-node-type="coder"');
    expect(out).toContain("board.tsx");
  });

  it("renders the mini as a nested node (its own data-node-type mini)", () => {
    const out = html({ model: baseModel(), selectedAgentId: null, onSelect: () => {} });
    expect(out).toContain('data-agent-id="m1"');
    expect(out).toContain('data-node-type="mini"');
    expect(out).toContain("card.tsx");
  });

  it("marks the selected node with data-selected", () => {
    const out = html({ model: baseModel(), selectedAgentId: "c1", onSelect: () => {} });
    // the selected node carries data-selected="true"
    expect(out).toMatch(/data-agent-id="c1"[^>]*data-selected="true"|data-selected="true"[^>]*data-agent-id="c1"/);
  });

  it("marks a live node and an idle node distinctly", () => {
    const out = html({ model: baseModel(), selectedAgentId: null, onSelect: () => {} });
    // c1 is running -> live; c2 is idle -> not live
    expect(out).toMatch(/data-agent-id="c1"[^>]*data-live="true"|data-live="true"[^>]*data-agent-id="c1"/);
    expect(out).toMatch(/data-agent-id="c2"[^>]*data-live="false"|data-live="false"[^>]*data-agent-id="c2"/);
  });

  it("marks a node with a pending question as asking (amber)", () => {
    const model = buildWorkConsoleModel({
      projectId: PROJECT,
      tasks: [tsk("t1", ["src/auth/login.ts"])],
      sessions: [
        sess({ agentId: "c1", currentTaskId: "t1",
          pendingQuestion: { id: "q", question: "which provider?", createdAt: "x" } }),
      ],
    });
    const out = html({ model, selectedAgentId: null, onSelect: () => {} });
    expect(out).toMatch(/data-agent-id="c1"[^>]*data-asks="true"|data-asks="true"[^>]*data-agent-id="c1"/);
  });

  it("marks a dirty node coral via dirtyAgentIds", () => {
    const out = html({
      model: baseModel(), selectedAgentId: null, onSelect: () => {},
      dirtyAgentIds: new Set(["c1"]),
    });
    expect(out).toMatch(/data-agent-id="c1"[^>]*data-dirty="true"|data-dirty="true"[^>]*data-agent-id="c1"/);
  });

  it("calls onSelect with the agentId when a node is clicked", () => {
    container = document.createElement("div");
    document.body.appendChild(container);
    let picked: string | null = null;
    root = createRoot(container);
    act(() => {
      root!.render(
        createElement(LivingPlan, {
          model: baseModel(),
          selectedAgentId: null,
          onSelect: (id: string) => { picked = id; },
        }),
      );
    });
    const node = container.querySelector('[data-agent-id="c1"]') as HTMLElement;
    expect(node).toBeTruthy();
    act(() => {
      node.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(picked).toBe("c1");
  });

  it("clicking a nested mini selects ONLY the mini, never the parent (no bubble)", () => {
    container = document.createElement("div");
    document.body.appendChild(container);
    const picks: string[] = [];
    root = createRoot(container);
    act(() => {
      root!.render(
        createElement(LivingPlan, {
          model: baseModel(),
          selectedAgentId: null,
          onSelect: (id: string) => { picks.push(id); },
        }),
      );
    });
    const mini = container.querySelector('[data-agent-id="m1"]') as HTMLElement;
    expect(mini).toBeTruthy();
    act(() => {
      mini.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    // exactly one selection, and it is the mini — the parent coder c1 must NOT also fire
    expect(picks).toEqual(["m1"]);
  });
});
