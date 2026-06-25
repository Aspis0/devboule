// @vitest-environment jsdom
//
// Phase B (board<->console twinning): the TaskCard gains a selection affordance so the
// bottom DAG board and the Work Console share ONE selection. Clicking a card selects its
// task (and, via the parent, the task's primary agent); the selected card shows a highlight
// ring. These tests pin the new `selected` styling + `onSelect` click, mirroring the repo's
// jsdom + createRoot + act interactive-test pattern (see SkillsView.test.tsx).

import { describe, it, expect, vi, afterEach } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { renderToStaticMarkup } from "react-dom/server";
import { TaskCard } from "./TaskCard";
import type { ProjectTask } from "../../types/backend";

(
  globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

function task(over: Partial<ProjectTask> = {}): ProjectTask {
  return {
    id: "t-1",
    title: "Wire the thing",
    status: "wip",
    priority: null,
    assignee: null,
    due: null,
    linkedResources: [],
    updatedAt: "",
    suspectFileIds: [],
    ...over,
  };
}

// All the gating props are computed by the parent; the card is presentation-only. Provide
// inert defaults so the tests focus on the new selection surface.
function baseProps() {
  return {
    agentControlled: false,
    workers: [],
    moveTargets: [],
    moveDisabled: true,
    manualMoveTitle: "",
    showLaunch: false,
    launchDisabled: true,
    launchTitle: "",
    coderDisabled: true,
    coderTitle: "",
    verifierDisabled: true,
    verifierTitle: "",
    manualDisabled: true,
    onMove: vi.fn(),
    onLaunchCoder: vi.fn(),
    onLaunchVerifier: vi.fn(),
    onCopyManualPrompt: vi.fn(),
  };
}

let root: Root | null = null;
let host: HTMLDivElement | null = null;
afterEach(() => {
  act(() => root?.unmount());
  root = null;
  host?.remove();
  host = null;
});

describe("TaskCard selection (twinning)", () => {
  it("shows a highlight ring + aria-current when selected", () => {
    const html = renderToStaticMarkup(
      createElement(TaskCard, { task: task(), ...baseProps(), selected: true }),
    );
    expect(html).toContain("ring-terracotta");
    expect(html).toContain('aria-current="true"');
  });

  it("renders no highlight ring when not selected", () => {
    const html = renderToStaticMarkup(
      createElement(TaskCard, { task: task(), ...baseProps(), selected: false }),
    );
    expect(html).not.toContain("ring-terracotta");
  });

  it("calls onSelect(task.id) when the card is clicked", () => {
    const onSelect = vi.fn();
    host = document.createElement("div");
    document.body.appendChild(host);
    root = createRoot(host);
    act(() => {
      root!.render(
        createElement(TaskCard, { task: task({ id: "t-42" }), ...baseProps(), onSelect }),
      );
    });
    const card = host.querySelector<HTMLElement>('[data-task-id="t-42"]');
    expect(card).not.toBeNull();
    act(() => {
      card!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(onSelect).toHaveBeenCalledTimes(1);
    expect(onSelect).toHaveBeenCalledWith("t-42");
  });

  it("does NOT select when an inner action button (Move/Launch menu) is clicked", () => {
    const onSelect = vi.fn();
    host = document.createElement("div");
    document.body.appendChild(host);
    root = createRoot(host);
    act(() => {
      root!.render(
        createElement(TaskCard, {
          task: task({ id: "t-7" }),
          ...baseProps(),
          // Render the Launch menu so there is a real <button> inside the card.
          showLaunch: true,
          launchDisabled: false,
          onSelect,
        }),
      );
    });
    const button = host.querySelector<HTMLButtonElement>("button");
    expect(button).not.toBeNull();
    act(() => {
      button!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("does not crash or require onSelect when the prop is omitted", () => {
    const html = renderToStaticMarkup(
      createElement(TaskCard, { task: task(), ...baseProps() }),
    );
    expect(html).toContain("Wire the thing");
  });
});
