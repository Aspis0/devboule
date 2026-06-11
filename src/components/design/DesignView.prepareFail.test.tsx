// @vitest-environment jsdom
//
// Regression test for the "stuck working card" BLOCKER: if runGenerate / runEdit
// throws AFTER pushing the working assistant card but BEFORE arming the stream
// (pendingRunRef never set), the card must flip to an error state with a Retry
// affordance — not spin forever. We force the throw by mocking buildGeneratePrompt /
// buildEditPrompt to throw (the exact point between the card push and the
// pendingRunRef assignment).

import { describe, it, expect, beforeEach, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { DesignProject } from "../../types/design";

// ---- backend mock ---------------------------------------------------------
const invokeSpy =
  vi.fn<(command: string, args?: Record<string, unknown>) => Promise<unknown>>(
    async () => undefined,
  );
vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (command: string, args?: Record<string, unknown>) =>
    invokeSpy(command, args),
  isTauriRuntime: () => true,
  useAppContext: () => ({ requestView: vi.fn() }),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }));

// ---- prompt builders mocked to THROW (the unguarded prepare failure) -------
vi.mock("./generation/prompt", () => ({
  buildGeneratePrompt: () => {
    throw new Error("prepare boom");
  },
  buildEditPrompt: () => {
    throw new Error("prepare boom");
  },
  buildRepairPrompt: () => null,
}));

// ---- controllable useDesignStream mock ------------------------------------
type StreamState = {
  text: string;
  status: "idle" | "streaming" | "done" | "error" | "cancelled";
  error: string | null;
};
const streamCtl: { state: StreamState; starts: string[]; notify: (() => void) | null } = {
  state: { text: "", status: "idle", error: null },
  starts: [],
  notify: null,
};
vi.mock("./useDesignStream", () => ({
  useDesignStream: () => ({
    text: streamCtl.state.text,
    status: streamCtl.state.status,
    error: streamCtl.state.error,
    start: (prompt: string) => {
      streamCtl.starts.push(prompt);
      streamCtl.state = { ...streamCtl.state, text: "" };
      streamCtl.notify?.();
    },
    cancel: () => {},
    reset: () => {},
  }),
}));

// ---- Canvas mock: expose select-first -------------------------------------
vi.mock("./canvas/DesignCanvas", () => ({
  DesignCanvas: (props: {
    project: DesignProject;
    onSelect: (id: string | null) => void;
  }) => {
    const firstId = Object.keys(props.project.manifest.nodes)[0] ?? null;
    return createElement("button", {
      type: "button",
      "data-testid": "select-first",
      onClick: () => props.onSelect(firstId),
    });
  },
}));

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let DesignView: typeof import("./DesignView").DesignView;
let rerender: (() => void) | null = null;

beforeEach(async () => {
  invokeSpy.mockClear();
  invokeSpy.mockImplementation(async () => undefined);
  streamCtl.state = { text: "", status: "idle", error: null };
  streamCtl.starts = [];
  streamCtl.notify = null;
  ({ DesignView } = await import("./DesignView"));
});

function render(): HTMLElement {
  const container = document.createElement("div");
  document.body.appendChild(container);
  let root!: Root;
  act(() => {
    root = createRoot(container);
    root.render(createElement(DesignView));
  });
  rerender = () => act(() => root.render(createElement(DesignView)));
  streamCtl.notify = rerender;
  return container;
}

function typePrompt(container: HTMLElement, value: string) {
  const ta = container.querySelector("textarea") as HTMLTextAreaElement;
  const setter = Object.getOwnPropertyDescriptor(
    window.HTMLTextAreaElement.prototype,
    "value",
  )!.set!;
  act(() => {
    setter.call(ta, value);
    ta.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

async function clickSend(container: HTMLElement) {
  const btn = container.querySelector(".send-btn") as HTMLButtonElement;
  await act(async () => {
    btn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

describe("DesignView — BLOCKER: prepare-failure card does not spin forever", () => {
  it("flips the working card to error with a Retry when generate prepare throws", async () => {
    const container = render();
    typePrompt(container, "make a hero");
    await clickSend(container);

    // No stream was started (the prepare threw before arming pendingRunRef).
    expect(streamCtl.starts.length).toBe(0);

    // The card is in an ERROR state, not a working spinner.
    const text = container.textContent ?? "";
    expect(text).toContain("Failed to start");
    expect(text).toContain("Settings");
    // A Retry affordance is present (the error card renders the rerun control).
    expect(text).toMatch(/Retry|Regenerate/i);
    // The working title is gone.
    expect(text).not.toContain("Generating…");
  });

  it("flips the edit card to error when edit prepare throws", async () => {
    const container = render();
    // Select the first demo node so the composer routes to the edit flow.
    const select = container.querySelector(
      "[data-testid=select-first]",
    ) as HTMLButtonElement;
    act(() => select.dispatchEvent(new MouseEvent("click", { bubbles: true })));

    typePrompt(container, "make it blue");
    await clickSend(container);

    expect(streamCtl.starts.length).toBe(0);
    const text = container.textContent ?? "";
    expect(text).toContain("Failed to start");
  });
});
