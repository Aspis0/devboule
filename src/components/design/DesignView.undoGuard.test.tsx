// @vitest-environment jsdom
//
// V7 regression: undo/redo (Ctrl+Z / Ctrl+Y) must be IGNORED while a generation/edit/
// Spot-Edit chain is live (panelBusy). Mutating the project via applySnapshot + persist
// mid-stream races the in-flight pipeline writing into the SAME project. These tests drive
// the stream hook's status to "streaming" and assert a Ctrl+Z fires NO design_save_project.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { act, createElement, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { DesignManifest, DesignProject } from "../../types/design";

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

const dialogCtl: { nextPick: string | null } = { nextPick: null };
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(async () => dialogCtl.nextPick),
}));

// Controllable stream status: a test flips `streamCtl.status` to "streaming" to simulate
// an in-flight generation, then re-renders the tree so DesignView recomputes panelBusy.
const streamCtl: { status: "idle" | "streaming" | "done" } = { status: "idle" };
vi.mock("./useDesignStream", () => ({
  useDesignStream: () => ({
    text: "",
    status: streamCtl.status,
    error: null,
    start: () => {},
    cancel: () => {},
    reset: () => {},
  }),
}));

let canvasProps: {
  project: DesignProject;
  onBeginChange: () => void;
  onManifestChange: (m: DesignManifest) => void;
} | null = null;
vi.mock("./canvas/DesignCanvas", () => ({
  DesignCanvas: (props: {
    project: DesignProject;
    onBeginChange: () => void;
    onManifestChange: (m: DesignManifest) => void;
  }) => {
    canvasProps = props;
    return createElement("div", { "data-testid": "canvas" });
  },
}));

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let DesignView: typeof import("./DesignView").DesignView;
let mountedRoots: Root[] = [];
let currentRoot: Root | null = null;

beforeEach(async () => {
  invokeSpy.mockClear();
  invokeSpy.mockImplementation(async () => undefined);
  dialogCtl.nextPick = null;
  canvasProps = null;
  streamCtl.status = "idle";
  mountedRoots = [];
  ({ DesignView } = await import("./DesignView"));
});

afterEach(() => {
  act(() => {
    for (const r of mountedRoots) r.unmount();
  });
  mountedRoots = [];
  currentRoot = null;
  document.body.innerHTML = "";
});

function render(): HTMLElement {
  const container = document.createElement("div");
  document.body.appendChild(container);
  act(() => {
    const root = createRoot(container);
    mountedRoots.push(root);
    currentRoot = root;
    root.render(createElement(StrictMode, null, createElement(DesignView)));
  });
  return container;
}

function rerender() {
  act(() => {
    currentRoot!.render(createElement(StrictMode, null, createElement(DesignView)));
  });
}

function findButton(container: HTMLElement, label: string): HTMLButtonElement {
  return Array.from(container.querySelectorAll("button")).find(
    (b) => b.textContent?.trim().startsWith(label),
  ) as HTMLButtonElement;
}

async function pickFolder(container: HTMLElement, path: string) {
  dialogCtl.nextPick = path;
  if (!container.querySelector(".pop.left")) {
    const proj = container.querySelector(".tb-proj") as HTMLButtonElement;
    act(() => proj.dispatchEvent(new MouseEvent("click", { bubbles: true })));
  }
  const btn = findButton(container, "Open working folder");
  await act(async () => {
    btn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

function makeOneEdit() {
  const props = canvasProps!;
  const m = props.project.manifest;
  const firstId = Object.keys(m.nodes)[0];
  act(() => {
    props.onBeginChange();
    props.onManifestChange({
      ...m,
      nodes: { ...m.nodes, [firstId]: { ...m.nodes[firstId], x: 999 } },
    });
  });
}

function pressCtrl(key: string, shiftKey = false) {
  act(() => {
    window.dispatchEvent(
      new KeyboardEvent("keydown", { key, ctrlKey: true, shiftKey, bubbles: true }),
    );
  });
}

function saveCount(): number {
  return invokeSpy.mock.calls.filter((c) => c[0] === "design_save_project").length;
}

describe("DesignView — V7 undo/redo blocked while a stream is live", () => {
  it("Ctrl+Z during an active stream does NOT applySnapshot/persist", async () => {
    const container = render();
    await pickFolder(container, "C:/work/design");
    makeOneEdit(); // create an undoable history entry
    invokeSpy.mockClear();

    // Simulate an in-flight generation: flip the stream status and re-render so
    // panelBusy (preparing || streaming || spotBusy) becomes true and the ref mirrors it.
    streamCtl.status = "streaming";
    rerender();

    pressCtrl("z");
    // The keydown handler early-returns on panelBusyRef → NO persist.
    expect(saveCount()).toBe(0);

    // Sanity: once the stream is done, undo works again (proves it was only GATED, not
    // permanently broken).
    streamCtl.status = "idle";
    rerender();
    invokeSpy.mockClear();
    pressCtrl("z");
    expect(saveCount()).toBe(1);
  });

  it("Ctrl+Y (redo) during an active stream is also ignored", async () => {
    const container = render();
    await pickFolder(container, "C:/work/design");
    makeOneEdit();
    pressCtrl("z"); // move the edit onto the redo branch (stream idle here)
    invokeSpy.mockClear();

    streamCtl.status = "streaming";
    rerender();

    pressCtrl("y");
    expect(saveCount()).toBe(0);
  });
});
