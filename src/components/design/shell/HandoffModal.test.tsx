// @vitest-environment jsdom
//
// HandoffModal presentational tests (Phase D): it renders nothing when closed; the
// packaging footer shows the "Packaging…" spinner; the dispatch phase exposes the
// project + client pickers and a guarded Dispatch button; the done phase shows the
// agent name + an "Open terminal" deep-link button that fires onOpenTerminal; and the
// scrim only closes when `closable`.

import { describe, it, expect, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { HandoffModal, type HandoffModalProps } from "./HandoffModal";
import type { HandoffStep } from "./useHandoff";
import type { ProjectSummary } from "../../../types/backend";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

function project(id: string, title: string): ProjectSummary {
  return {
    id,
    title,
    status: "active",
    updatedAt: "",
    rootPath: "C:/repo",
    revision: "r",
    path: `projects/${id}.md`,
    taskCounts: {} as unknown as ProjectSummary["taskCounts"],
    gitStatus: {} as unknown as ProjectSummary["gitStatus"],
  };
}

const STEPS: HandoffStep[] = [
  { id: "save", label: "Save to repo", detail: "x", status: "done", icon: "save", agent: "design" },
  { id: "export", label: "Export layouts", detail: "x", status: "done", icon: "code", agent: "design" },
  { id: "contract", label: "Design contract", detail: "x", status: "done", icon: "fileText", agent: "design" },
  { id: "capture", label: "Capture preview", detail: "x", status: "done", icon: "camera", agent: "design" },
  { id: "dispatch", label: "Coder agent", detail: "x", status: "idle", icon: "cpu", agent: "coder agent" },
];

function baseProps(over: Partial<HandoffModalProps> = {}): HandoffModalProps {
  return {
    open: true,
    workingFolderPath: "C:/repo/.devboule-design/landing",
    phase: "dispatch",
    steps: STEPS,
    flow: { repoDone: true, agentsStarted: false, done: false },
    projects: [project("repo", "Repo One")],
    projectsError: null,
    selectedProjectId: "repo",
    client: "claude",
    agentId: null,
    errorStage: null,
    errorMessage: null,
    dispatching: false,
    canDispatch: true,
    closable: true,
    onClose: vi.fn(),
    onSelectProject: vi.fn(),
    onSelectClient: vi.fn(),
    onRetryPackaging: vi.fn(),
    onDispatch: vi.fn(),
    onOpenTerminal: vi.fn(),
    ...over,
  };
}

function render(props: HandoffModalProps): HTMLElement {
  const container = document.createElement("div");
  document.body.appendChild(container);
  act(() => {
    createRoot(container).render(createElement(HandoffModal, props));
  });
  return container;
}

describe("HandoffModal", () => {
  it("renders nothing when closed", () => {
    const c = render(baseProps({ open: false }));
    expect(c.querySelector('[data-testid="handoff-modal"]')).toBeNull();
  });

  it("packaging phase shows the packaging spinner footer and no dispatch controls", () => {
    const c = render(
      baseProps({ phase: "packaging", flow: { repoDone: false, agentsStarted: false, done: false } }),
    );
    expect(c.textContent).toContain("Packaging project");
    expect(c.querySelector('[data-testid="handoff-dispatch-controls"]')).toBeNull();
  });

  it("dispatch phase exposes the project + client pickers and the Dispatch button", () => {
    const onDispatch = vi.fn();
    const c = render(baseProps({ onDispatch }));
    const projectSel = c.querySelector(
      '[data-testid="handoff-project-select"]',
    ) as HTMLSelectElement;
    const clientSel = c.querySelector(
      '[data-testid="handoff-client-select"]',
    ) as HTMLSelectElement;
    expect(projectSel.value).toBe("repo");
    expect(clientSel.value).toBe("claude");
    const dispatch = c.querySelector(
      '[data-testid="handoff-dispatch"]',
    ) as HTMLButtonElement;
    expect(dispatch.disabled).toBe(false);
    act(() => dispatch.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(onDispatch).toHaveBeenCalledTimes(1);
  });

  it("disables the Dispatch button when canDispatch is false", () => {
    const c = render(baseProps({ canDispatch: false }));
    const dispatch = c.querySelector(
      '[data-testid="handoff-dispatch"]',
    ) as HTMLButtonElement;
    expect(dispatch.disabled).toBe(true);
  });

  it("done phase shows the agent name and Open terminal deep-links", () => {
    const onOpenTerminal = vi.fn();
    const c = render(
      baseProps({
        phase: "done",
        agentId: "coder-99",
        flow: { repoDone: true, agentsStarted: true, done: true },
        onOpenTerminal,
      }),
    );
    expect(c.textContent).toContain("coder-99");
    const open = c.querySelector(
      '[data-testid="handoff-open-terminal"]',
    ) as HTMLButtonElement;
    act(() => open.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(onOpenTerminal).toHaveBeenCalledTimes(1);
  });

  it("the scrim closes the modal only when closable", () => {
    const onClose = vi.fn();
    const c = render(baseProps({ closable: false, dispatching: true, onClose }));
    const scrim = c.querySelector('[data-testid="handoff-modal"]') as HTMLElement;
    act(() => scrim.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(onClose).not.toHaveBeenCalled();
    // No close button is rendered while not closable.
    expect(c.querySelector(".ho-close")).toBeNull();
  });

  it("surfaces an error row with a Retry that routes by stage", () => {
    const onDispatch = vi.fn();
    const onRetryPackaging = vi.fn();
    const c = render(
      baseProps({
        errorStage: "dispatch",
        errorMessage: "claude not found in PATH",
        onDispatch,
        onRetryPackaging,
      }),
    );
    expect(c.querySelector('[data-testid="handoff-error"]')?.textContent).toContain(
      "claude not found",
    );
    const retry = c.querySelector('[data-testid="handoff-retry"]') as HTMLButtonElement;
    act(() => retry.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(onDispatch).toHaveBeenCalledTimes(1);
    expect(onRetryPackaging).not.toHaveBeenCalled();
  });

  it("routes Retry to onRetryPackaging (not onDispatch) when the error stage is packaging", () => {
    const onDispatch = vi.fn();
    const onRetryPackaging = vi.fn();
    const c = render(
      baseProps({
        phase: "packaging",
        flow: { repoDone: false, agentsStarted: false, done: false },
        errorStage: "packaging",
        errorMessage: "Save failed — could not consolidate the project to disk.",
        onDispatch,
        onRetryPackaging,
      }),
    );
    const retry = c.querySelector('[data-testid="handoff-retry"]') as HTMLButtonElement;
    act(() => retry.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(onRetryPackaging).toHaveBeenCalledTimes(1);
    expect(onDispatch).not.toHaveBeenCalled();
  });

  it("surfaces a project-load error near the selector", () => {
    const c = render(
      baseProps({ projectsError: "Could not load projects — close and reopen." }),
    );
    const err = c.querySelector('[data-testid="handoff-projects-error"]');
    expect(err?.textContent).toContain("Could not load projects");
  });

  it("shows no project-load error when projectsError is null", () => {
    const c = render(baseProps());
    expect(c.querySelector('[data-testid="handoff-projects-error"]')).toBeNull();
  });
});
