// @vitest-environment jsdom
//
// WorkConsole ties the Living Plan (left) to the Focus stage (right) for the selected node,
// derives the model from sessions+tasks, and routes the composer to the right backend command.
// useAgentConsole degrades to empty when the Tauri runtime is absent (tests), so this mounts
// without a backend and asserts the structure + selection wiring.

import { describe, it, expect, afterEach } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { WorkConsole } from "./WorkConsole";
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

const sessions = [
  sess({ agentId: "c1", currentTaskId: "t-board", client: "codex" }),
  sess({ agentId: "c2", currentTaskId: "t-login" }),
];
const tasks = [
  tsk("t-board", ["src/views/projects/board.tsx"]),
  tsk("t-login", ["src/auth/login.ts"]),
];

type Props = Parameters<typeof WorkConsole>[0];
const baseProps = (over: Partial<Props> = {}): Props => ({
  sessions, tasks, projectId: PROJECT,
  ptyAgentIds: new Set<string>(),
  selectedAgentId: "c1",
  onSelectAgent: () => {},
  ...over,
});

let root: Root | null = null;
let container: HTMLDivElement | null = null;
function mount(props: Props) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(createElement(WorkConsole, props)));
  return container;
}
afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  if (container) container.remove();
  container = null;
});

describe("WorkConsole", () => {
  it("renders the Living Plan districts and the Focus stage for the selected node", () => {
    const c = mount(baseProps({ selectedAgentId: "c2" }));
    expect(c.querySelector('[data-district="projects"]')).toBeTruthy();
    expect(c.querySelector('[data-district="auth"]')).toBeTruthy();
    // FocusStage header shows the selected node's file (c2 -> login.ts)
    expect(c.innerHTML).toContain("login.ts");
  });

  it("marks the selected node in the Living Plan", () => {
    const c = mount(baseProps({ selectedAgentId: "c1" }));
    const sel = c.querySelector('[data-agent-id="c1"]') as HTMLElement;
    expect(sel?.getAttribute("data-selected")).toBe("true");
  });

  it("calls onSelectAgent when a Living Plan node is clicked", () => {
    const picks: string[] = [];
    const c = mount(baseProps({ selectedAgentId: "c1", onSelectAgent: (id) => picks.push(id) }));
    const node = c.querySelector('[data-agent-id="c2"]') as HTMLElement;
    act(() => node.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(picks).toEqual(["c2"]);
  });

  it("shows a placeholder when nothing is selected", () => {
    const c = mount(baseProps({ selectedAgentId: null }));
    // still renders the Living Plan, but no focus header file
    expect(c.querySelector('[data-district="auth"]')).toBeTruthy();
    expect(c.querySelector('[data-view]')).toBeFalsy();
  });
});
