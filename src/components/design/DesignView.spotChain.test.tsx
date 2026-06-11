// @vitest-environment jsdom
//
// Regression tests for the Spot Edit chain CONTROL fixes (A3 hostile review #2):
//   (a) the composer is BUSY for the whole chain (including the inter-node gap), so a
//       send can't interleave a generate/edit into the chain's single stream slot;
//   (b) Stop during the inter-node gap (no live stream) reliably aborts the chain —
//       no further chain stream starts;
//   (c) cancel-then-new-region: the second chain runs to completion (the stale
//       advance-schedule flag from the aborted chain must not stall it).
//
// We mock BOTH DesignCanvas (to drive onRegionAnalyze) and AssistantPanel (to read
// `busy` and invoke `onSend` / `onStop`), plus the same controllable useDesignStream
// mock the other suites use.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { DesignProject, Point } from "../../types/design";

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

// ---- controllable useDesignStream mock ------------------------------------
type StreamState = {
  text: string;
  status: "idle" | "streaming" | "done" | "error" | "cancelled";
  error: string | null;
};
const streamCtl: {
  state: StreamState;
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
      streamCtl.state = { ...streamCtl.state, text: "", status: "streaming" };
      streamCtl.notify?.();
    },
    cancel: () => {
      streamCtl.state = { ...streamCtl.state, status: "cancelled" };
      streamCtl.notify?.();
    },
    reset: () => {},
  }),
}));

// ---- Canvas harness mock --------------------------------------------------
let lastProject: DesignProject | null = null;
const canvasCb: {
  onRegionAnalyze?: (pts: Point[], prompt: string) => void;
} = {};
vi.mock("./canvas/DesignCanvas", () => ({
  DesignCanvas: (props: {
    project: DesignProject;
    onRegionAnalyze?: (pts: Point[], prompt: string) => void;
  }) => {
    lastProject = props.project;
    canvasCb.onRegionAnalyze = props.onRegionAnalyze;
    return createElement("div", { "data-testid": "canvas-harness" });
  },
}));

// ---- AssistantPanel harness mock: read busy, invoke onSend / onStop --------
const panelCb: {
  busy?: boolean;
  onSend?: (text: string) => void;
  onStop?: () => void;
} = {};
vi.mock("./panel/AssistantPanel", () => ({
  AssistantPanel: (props: {
    busy: boolean;
    onSend: (text: string) => void;
    onStop: () => void;
  }) => {
    panelCb.busy = props.busy;
    panelCb.onSend = props.onSend;
    panelCb.onStop = props.onStop;
    return createElement("div", { "data-testid": "panel-harness" });
  },
}));

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let DesignView: typeof import("./DesignView").DesignView;
let rerender: (() => void) | null = null;

const DEFAULT_DESIGN_MD = "# Contract\nFollow the house style.";

function twoNodeProject(): DesignProject {
  return {
    meta: {
      schemaVersion: 1,
      id: "p",
      name: "Loaded",
      createdAt: "1970-01-01T00:00:00Z",
      updatedAt: "1970-01-01T00:00:00Z",
      canvas: { w: 1440, h: 1024, grid: 8 },
      nodeOrder: ["hero", "cta"],
    },
    manifest: {
      schemaVersion: 1,
      nodes: {
        hero: { x: 0, y: 0, z: 1, w: 400, h: 200, kind: "html" },
        cta: { x: 0, y: 220, z: 2, w: 400, h: 80, kind: "html" },
      },
    },
    components: {
      hero: '<section data-node-id="hero"><h1>Hero</h1></section>',
      cta: '<button data-node-id="cta">Go</button>',
    },
  };
}

beforeEach(async () => {
  invokeSpy.mockClear();
  invokeSpy.mockImplementation(async (command: string) => {
    if (command === "design_read_design_md") return DEFAULT_DESIGN_MD;
    if (command === "design_load_project") return twoNodeProject();
    return undefined;
  });
  streamCtl.state = { text: "", status: "idle", error: null };
  streamCtl.starts = [];
  streamCtl.notify = null;
  lastProject = null;
  canvasCb.onRegionAnalyze = undefined;
  panelCb.busy = undefined;
  panelCb.onSend = undefined;
  panelCb.onStop = undefined;
  ({ DesignView } = await import("./DesignView"));
});

afterEach(() => {
  document.body.innerHTML = "";
  rerender = null;
  vi.restoreAllMocks();
});

function render(): { container: HTMLElement } {
  const container = document.createElement("div");
  document.body.appendChild(container);
  let root!: Root;
  act(() => {
    root = createRoot(container);
    root.render(createElement(DesignView));
  });
  rerender = () => act(() => root.render(createElement(DesignView)));
  streamCtl.notify = rerender;
  return { container };
}

function emitDone(text: string) {
  act(() => {
    streamCtl.state = { text, status: "done", error: null };
    rerender?.();
  });
}

async function loadProject(container: HTMLElement) {
  const { open } = await import("@tauri-apps/plugin-dialog");
  (open as unknown as ReturnType<typeof vi.fn>).mockResolvedValueOnce("C:/proj");
  const proj = container.querySelector(".tb-proj") as HTMLButtonElement;
  act(() => proj.dispatchEvent(new MouseEvent("click", { bubbles: true })));
  const pickBtn = Array.from(container.querySelectorAll("button")).find((b) =>
    b.textContent?.includes("Open working folder"),
  ) as HTMLButtonElement;
  await act(async () => {
    pickBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    for (let i = 0; i < 14; i++) await Promise.resolve();
  });
  rerender?.();
}

// A region covering BOTH nodes (0..400 x 0..300).
const REGION_BOTH: Point[] = [
  { x: -10, y: -10 },
  { x: 410, y: -10 },
  { x: 410, y: 310 },
  { x: -10, y: 310 },
];

describe("DesignView — Spot Edit chain control", () => {
  it("composer is busy for the WHOLE chain (a send is blocked mid-chain)", async () => {
    const { container } = render();
    await loadProject(container);
    expect(panelCb.busy).toBe(false);

    await act(async () => {
      canvasCb.onRegionAnalyze!(REGION_BOTH, "tighten spacing");
      for (let i = 0; i < 6; i++) await Promise.resolve();
    });
    // First node streaming → panel busy.
    expect(panelCb.busy).toBe(true);
    expect(streamCtl.starts).toHaveLength(1);

    // Finish the first node. Even in the inter-node GAP (status `done`, before the 2nd
    // stream starts) the panel must stay busy so a send can't interleave.
    emitDone('<section data-node-id="hero"><h1>Hero</h1></section>');
    expect(panelCb.busy).toBe(true);

    await act(async () => {
      for (let i = 0; i < 6; i++) await Promise.resolve();
    });
    // Second node streaming → still busy, second stream started.
    expect(panelCb.busy).toBe(true);
    expect(streamCtl.starts).toHaveLength(2);
  });

  it("Stop during the inter-node gap aborts the chain — no further chain stream", async () => {
    const { container } = render();
    await loadProject(container);

    await act(async () => {
      canvasCb.onRegionAnalyze!(REGION_BOTH, "tighten spacing");
      for (let i = 0; i < 6; i++) await Promise.resolve();
    });
    expect(streamCtl.starts).toHaveLength(1);

    // First node done → we are in the inter-node gap (no live stream yet).
    emitDone('<section data-node-id="hero"><h1>Hero</h1></section>');
    // STOP before flushing the advance microtask.
    act(() => panelCb.onStop!());

    await act(async () => {
      for (let i = 0; i < 8; i++) await Promise.resolve();
    });
    // The second node never streamed; the chain was torn down.
    expect(streamCtl.starts).toHaveLength(1);
    expect(panelCb.busy).toBe(false);
  });

  it("cancel-then-new-region: the second chain runs to completion (no stale-schedule stall)", async () => {
    const { container } = render();
    await loadProject(container);

    // First chain, then Stop in the gap.
    await act(async () => {
      canvasCb.onRegionAnalyze!(REGION_BOTH, "first");
      for (let i = 0; i < 6; i++) await Promise.resolve();
    });
    expect(streamCtl.starts).toHaveLength(1);
    emitDone('<section data-node-id="hero"><h1>Hero</h1></section>');
    act(() => panelCb.onStop!());
    await act(async () => {
      for (let i = 0; i < 8; i++) await Promise.resolve();
    });
    expect(streamCtl.starts).toHaveLength(1);

    // Reset the stream back to idle (a real cancel settles), then a NEW region/chain.
    act(() => {
      streamCtl.state = { text: "", status: "idle", error: null };
      rerender?.();
    });

    await act(async () => {
      canvasCb.onRegionAnalyze!(REGION_BOTH, "second");
      for (let i = 0; i < 6; i++) await Promise.resolve();
    });
    // Second chain's first node streamed.
    expect(streamCtl.starts).toHaveLength(2);
    expect(streamCtl.starts[1]).toContain("Spot edit (region selection): second");

    // Drive it to completion across BOTH nodes (this is the path that would stall if the
    // advance-schedule flag were left stuck `true` by the aborted first chain).
    emitDone('<section data-node-id="hero"><h1>Hero</h1></section>');
    await act(async () => {
      for (let i = 0; i < 8; i++) await Promise.resolve();
    });
    expect(streamCtl.starts).toHaveLength(3); // second node of the second chain ran

    emitDone('<button data-node-id="cta">Go</button>');
    await act(async () => {
      for (let i = 0; i < 8; i++) await Promise.resolve();
    });
    expect(streamCtl.starts).toHaveLength(3); // queue drained, no extra stream
    expect(panelCb.busy).toBe(false);
    expect(lastProject).toBeTruthy();
  });
});
