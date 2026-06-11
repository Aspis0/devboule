// @vitest-environment jsdom
//
// DesignView-side tests for Phase A3:
//   - content-edit COMMIT re-sanitizes (an onerror/class injected during CE is
//     stripped) and calls design_write_node with the SAME payload shape as the edit
//     pipeline (node markup first, then design_write_manifest);
//   - Spot Edit: onRegionAnalyze with 2 hit nodes runs TWO SEQUENTIAL runEdit streams
//     (the 2nd starts only after the 1st's `done`), an empty region toasts, and an
//     empty prompt uses the fixed auto-detect instruction.
//
// The Canvas is mocked to a thin harness exposing the new callbacks
// (onNodeMarkupCommit / onRegionAnalyze) plus a select-first button, so the test can
// invoke them directly. `useDesignStream` is the same controllable mock the
// generation suite uses (records prompts, drives status/text).

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

// ---- Canvas harness mock: expose the A3 callbacks -------------------------
let lastProject: DesignProject | null = null;
const canvasCb: {
  onNodeMarkupCommit?: (id: string, raw: string) => void;
  onRegionAnalyze?: (pts: Point[], prompt: string) => void;
  onSelect?: (id: string | null) => void;
} = {};
vi.mock("./canvas/DesignCanvas", () => ({
  DesignCanvas: (props: {
    project: DesignProject;
    onSelect: (id: string | null) => void;
    onNodeMarkupCommit?: (id: string, raw: string) => void;
    onRegionAnalyze?: (pts: Point[], prompt: string) => void;
  }) => {
    lastProject = props.project;
    canvasCb.onNodeMarkupCommit = props.onNodeMarkupCommit;
    canvasCb.onRegionAnalyze = props.onRegionAnalyze;
    canvasCb.onSelect = props.onSelect;
    return createElement("div", { "data-testid": "canvas-harness" });
  },
}));

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let DesignView: typeof import("./DesignView").DesignView;
let rerender: (() => void) | null = null;

const DEFAULT_DESIGN_MD = "# Contract\nFollow the house style.";

// A loaded project with TWO nodes laid out so a region can overlap both.
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
  canvasCb.onNodeMarkupCommit = undefined;
  canvasCb.onRegionAnalyze = undefined;
  ({ DesignView } = await import("./DesignView"));
});

afterEach(() => {
  document.body.innerHTML = "";
  rerender = null;
  vi.restoreAllMocks();
});

function render(): { container: HTMLElement; root: Root } {
  const container = document.createElement("div");
  document.body.appendChild(container);
  let root!: Root;
  act(() => {
    root = createRoot(container);
    root.render(createElement(DesignView));
  });
  rerender = () => act(() => root.render(createElement(DesignView)));
  streamCtl.notify = rerender;
  return { container, root };
}

function emitDone(text: string) {
  act(() => {
    streamCtl.state = { text, status: "done", error: null };
    rerender?.();
  });
}

// Load the two-node project via the ProjectPopover "Open working folder…" flow.
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

describe("DesignView — content-edit commit", () => {
  it("re-sanitizes the serialized markup (strips onerror/class) and writes node then manifest", async () => {
    const { container } = render();
    await loadProject(container);
    expect(canvasCb.onNodeMarkupCommit).toBeTruthy();

    invokeSpy.mockClear();
    // Simulate the canvas handing up RAW serialized markup that (maliciously) carries
    // an injected onerror + class — the CE commit MUST re-sanitize before persisting.
    await act(async () => {
      canvasCb.onNodeMarkupCommit!(
        "hero",
        '<section data-node-id="hero" class="leak"><img src="x" onerror="alert(1)"><h1>Hero edited</h1></section>',
      );
      await Promise.resolve();
      await Promise.resolve();
    });

    const writeNode = invokeSpy.mock.calls.find(
      (c) => c[0] === "design_write_node",
    );
    expect(writeNode).toBeTruthy();
    const payload = writeNode![1] as {
      workingFolderPath: string;
      nodeId: string;
      markup: string;
    };
    expect(payload.nodeId).toBe("hero");
    expect(payload.workingFolderPath).toBe("C:/proj");
    expect(payload.markup.toLowerCase()).not.toContain("onerror");
    expect(payload.markup.toLowerCase()).not.toContain("alert(1)");
    expect(payload.markup).not.toContain("leak"); // class stripped
    expect(payload.markup).toContain("Hero edited");
    expect(payload.markup).toContain('data-node-id="hero"'); // id re-stamped

    // Manifest is written AFTER the node (same serialized path as the edit pipeline).
    const order = invokeSpy.mock.calls.map((c) => c[0]);
    expect(order.indexOf("design_write_node")).toBeLessThan(
      order.indexOf("design_write_manifest"),
    );

    // The in-memory project also reflects the sanitized markup.
    expect(lastProject?.components.hero).toContain("Hero edited");
    expect((lastProject?.components.hero ?? "").toLowerCase()).not.toContain(
      "onerror",
    );
  });

  it("a 2-root serialization is preserved by wrapping both roots under one <div>", async () => {
    const { container } = render();
    await loadProject(container);
    invokeSpy.mockClear();
    // CE hands up TWO top-level roots (the wrapping <section> was deleted, hoisting its
    // children). Both must survive — wrapped under a single re-anchorable root.
    await act(async () => {
      canvasCb.onNodeMarkupCommit!(
        "hero",
        "<h1>First root</h1><p>Second root</p>",
      );
      await Promise.resolve();
      await Promise.resolve();
    });
    const writeNode = invokeSpy.mock.calls.find(
      (c) => c[0] === "design_write_node",
    );
    expect(writeNode).toBeTruthy();
    const payload = writeNode![1] as { markup: string };
    // BOTH roots' content is retained (not just the first).
    expect(payload.markup).toContain("First root");
    expect(payload.markup).toContain("Second root");
    // The id is stamped on the single wrapping root.
    expect(payload.markup).toContain('data-node-id="hero"');
    expect(lastProject?.components.hero).toContain("Second root");
  });

  it("a no-op CE exit (serialized markup equals the stored markup) writes nothing", async () => {
    const { container } = render();
    await loadProject(container);
    // Snapshot the message count BEFORE (history push surfaces as a status/message).
    invokeSpy.mockClear();
    const beforeProject = lastProject;
    // Hand up the EXACT stored markup for `hero` (a pure click, no edit). The fragile
    // comparison fix normalizes both sides, so this must NOT persist or push history.
    await act(async () => {
      canvasCb.onNodeMarkupCommit!(
        "hero",
        '<section data-node-id="hero"><h1>Hero</h1></section>',
      );
      await Promise.resolve();
      await Promise.resolve();
    });
    const wroteNode = invokeSpy.mock.calls.some(
      (c) => c[0] === "design_write_node",
    );
    const wroteManifest = invokeSpy.mock.calls.some(
      (c) => c[0] === "design_write_manifest",
    );
    expect(wroteNode).toBe(false);
    expect(wroteManifest).toBe(false);
    // The in-memory project reference is unchanged (no setProject → no history push).
    expect(lastProject).toBe(beforeProject);
  });
});

describe("DesignView — Spot Edit sequential chain", () => {
  it("runs TWO sequential runEdit streams (2nd starts only after the 1st's done)", async () => {
    const { container } = render();
    await loadProject(container);
    expect(canvasCb.onRegionAnalyze).toBeTruthy();

    // A region covering BOTH nodes (0..400 x 0..300).
    const region: Point[] = [
      { x: -10, y: -10 },
      { x: 410, y: -10 },
      { x: 410, y: 310 },
      { x: -10, y: 310 },
    ];
    await act(async () => {
      canvasCb.onRegionAnalyze!(region, "tighten spacing");
      for (let i = 0; i < 6; i++) await Promise.resolve();
    });

    // Exactly ONE stream started so far (the first node) — the chain is sequential.
    expect(streamCtl.starts).toHaveLength(1);
    expect(streamCtl.starts[0]).toContain("Spot edit (region selection): tighten spacing");

    // Finish the first node's stream — the chain then starts the second.
    emitDone('<section data-node-id="hero"><h1>Hero</h1></section>');
    await act(async () => {
      for (let i = 0; i < 6; i++) await Promise.resolve();
    });
    expect(streamCtl.starts).toHaveLength(2);

    // Finish the second; no third stream starts (queue drained).
    emitDone('<button data-node-id="cta">Go</button>');
    await act(async () => {
      for (let i = 0; i < 6; i++) await Promise.resolve();
    });
    expect(streamCtl.starts).toHaveLength(2);
  });

  it("an empty region toasts 'No sections in the region' and starts no stream", async () => {
    const { container } = render();
    await loadProject(container);
    // A region far away from both nodes.
    const region: Point[] = [
      { x: 2000, y: 2000 },
      { x: 2100, y: 2000 },
      { x: 2100, y: 2100 },
      { x: 2000, y: 2100 },
    ];
    await act(async () => {
      canvasCb.onRegionAnalyze!(region, "fix it");
      for (let i = 0; i < 4; i++) await Promise.resolve();
    });
    expect(streamCtl.starts).toHaveLength(0);
    expect(container.textContent).toContain("No sections in the region");
  });

  it("an empty prompt uses the fixed auto-detect instruction", async () => {
    const { container } = render();
    await loadProject(container);
    const region: Point[] = [
      { x: -10, y: -10 },
      { x: 410, y: -10 },
      { x: 410, y: 100 }, // overlaps only the hero
      { x: -10, y: 100 },
    ];
    await act(async () => {
      canvasCb.onRegionAnalyze!(region, "");
      for (let i = 0; i < 6; i++) await Promise.resolve();
    });
    expect(streamCtl.starts).toHaveLength(1);
    expect(streamCtl.starts[0]).toContain(
      "Fix off-token colors, contrast and spacing inconsistencies in this section",
    );
  });
});
