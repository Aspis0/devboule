// @vitest-environment jsdom
//
// DesignView ↔ panel integration: the assistant-panel resizer clamp and the
// suggestion→draft→send round-trip through the REAL composer wiring. Uses the same
// raw-DOM + mocked-stream harness as DesignView.generation.test.tsx.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { DesignProject } from "../../types/design";

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

const streamCtl: {
  state: { text: string; status: string; error: string | null };
  starts: string[];
  notify: (() => void) | null;
} = { state: { text: "", status: "idle", error: null }, starts: [], notify: null };

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

function render() {
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

function pointer(el: Element, type: string, clientX: number) {
  act(() =>
    el.dispatchEvent(
      new PointerEvent(type, { bubbles: true, clientX, pointerId: 1 }),
    ),
  );
}

describe("DesignView — assistant panel resizer", () => {
  it("clamps the panel width to [290, 540] under a drag", () => {
    const container = render();
    const aside = container.querySelector(".assist") as HTMLElement;
    expect(aside.style.width).toBe("350px"); // default

    const resizer = container.querySelector(".panel-resizer") as HTMLElement;
    // Stub pointer-capture (jsdom doesn't implement it).
    (resizer as unknown as { setPointerCapture: () => void }).setPointerCapture =
      () => {};
    (resizer as unknown as { releasePointerCapture: () => void }).releasePointerCapture =
      () => {};
    (resizer as unknown as { hasPointerCapture: () => boolean }).hasPointerCapture =
      () => false;

    // Drag far LEFT (negative dx) — the right-anchored panel grows, clamped at 540.
    pointer(resizer, "pointerdown", 1000);
    pointer(resizer, "pointermove", 0); // dx = -1000 -> 350 + 1000, clamp 540
    expect(aside.style.width).toBe("540px");
    pointer(resizer, "pointerup", 0);

    // Drag far RIGHT (positive dx) — the panel shrinks, clamped at 290.
    pointer(resizer, "pointerdown", 0);
    pointer(resizer, "pointermove", 1000); // dx = +1000 -> 540 - 1000, clamp 290
    expect(aside.style.width).toBe("290px");
    pointer(resizer, "pointerup", 1000);
  });
});

describe("DesignView — suggestion seeds the composer then sends a generate", () => {
  it("clicking a suggestion fills the draft; sending generates with that text", async () => {
    invokeSpy.mockImplementation(async (command: string) => {
      if (command === "get_design_llm_backend") return { kind: "ollama", model: "q" };
      if (command === "design_oracle_context") return [];
      return undefined;
    });
    const container = render();

    // A suggestion seeds the composer draft (does not send).
    const sugg = container.querySelector(".sugg") as HTMLButtonElement;
    act(() => sugg.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    const ta = container.querySelector("textarea") as HTMLTextAreaElement;
    expect(ta.value).toBe("A pricing section coherent with our app");
    expect(streamCtl.starts.length).toBe(0);

    // Pressing send streams a GENERATE whose prompt carries the seeded text.
    const send = container.querySelector(".send-btn") as HTMLButtonElement;
    await act(async () => {
      send.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(streamCtl.starts.length).toBe(1);
    expect(streamCtl.starts[0]).toContain("A pricing section coherent with our app");
  });
});
