// @vitest-environment jsdom
//
// B1+M6+W4 regression: undo/redo must run their persisting side effect
// (applySnapshot -> persistProject -> design_save_project) IMPERATIVELY, OUTSIDE any
// setState updater. The old code called applySnapshot inside `setHistory(h => …)`,
// so React.StrictMode (which intentionally double-invokes state updaters in dev) and
// rapid Ctrl+Z fired the Tauri IPC write TWICE per undo. These tests render under
// <StrictMode> and assert exactly ONE design_save_project per undo()/redo().

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { act, createElement, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { DesignManifest, DesignProject } from "../../types/design";

// ---- backend mock ---------------------------------------------------------
const invokeSpy =
  vi.fn<(command: string, args?: Record<string, unknown>) => Promise<unknown>>(
    async () => undefined,
  );

vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (command: string, args?: Record<string, unknown>) =>
    invokeSpy(command, args),
  isTauriRuntime: () => true,
}));

// ---- native folder picker mock --------------------------------------------
const dialogCtl: { nextPick: string | null } = { nextPick: null };
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(async () => dialogCtl.nextPick),
}));

// ---- stub the streaming hook (not under test) -----------------------------
vi.mock("./useDesignStream", () => ({
  useDesignStream: () => ({
    text: "",
    status: "idle" as const,
    error: null,
    start: () => {},
    cancel: () => {},
    reset: () => {},
  }),
}));

// ---- Canvas mock: expose onBeginChange + onManifestChange so a test can drive
// ---- one interactive edit (begin-change snapshot, then a manifest mutation). --
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
// Track every mounted root so afterEach can UNMOUNT it. A leaked root keeps its
// window `keydown` listener alive, so a later test's Ctrl+Z/Y would be handled by
// BOTH the leaked instance and the new one — double-counting saves. This is a TEST
// hygiene requirement, not a product bug.
let mountedRoots: Root[] = [];

beforeEach(async () => {
  invokeSpy.mockClear();
  invokeSpy.mockImplementation(async () => undefined);
  dialogCtl.nextPick = null;
  canvasProps = null;
  mountedRoots = [];
  ({ DesignView } = await import("./DesignView"));
});

afterEach(() => {
  act(() => {
    for (const r of mountedRoots) r.unmount();
  });
  mountedRoots = [];
  document.body.innerHTML = "";
});

function render(): HTMLElement {
  const container = document.createElement("div");
  document.body.appendChild(container);
  act(() => {
    // StrictMode double-invokes render + state updaters in dev — exactly the
    // condition that double-fired the old in-updater applySnapshot.
    const root = createRoot(container);
    mountedRoots.push(root);
    root.render(createElement(StrictMode, null, createElement(DesignView)));
  });
  return container;
}

function findButton(container: HTMLElement, label: string): HTMLButtonElement {
  return Array.from(container.querySelectorAll("button")).find(
    (b) => b.textContent?.trim().startsWith(label),
  ) as HTMLButtonElement;
}

async function pickFolder(container: HTMLElement, path: string) {
  dialogCtl.nextPick = path;
  const btn = findButton(container, "Choose folder");
  await act(async () => {
    btn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

/** Drive one interactive edit through the canvas props: snapshot then mutate. */
function makeOneEdit() {
  const props = canvasProps!;
  const m = props.project.manifest;
  const firstId = Object.keys(m.nodes)[0];
  act(() => {
    props.onBeginChange(); // push pre-edit snapshot onto history
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

describe("DesignView — undo/redo persist exactly once under StrictMode (B1+M6+W4)", () => {
  it("one Ctrl+Z triggers exactly ONE design_save_project (no StrictMode double-fire)", async () => {
    const container = render();
    expect(canvasProps).toBeTruthy();

    // Set a working folder so persistProject actually invokes the backend.
    await pickFolder(container, "C:/work/design");
    makeOneEdit();
    invokeSpy.mockClear(); // ignore the edit's own throttled write; isolate undo

    pressCtrl("z");

    // The undo's applySnapshot -> persistProject fires design_save_project ONCE.
    // Under the old in-updater code, StrictMode double-invoked the updater and this
    // was 2.
    expect(saveCount()).toBe(1);
  });

  it("Ctrl+Z then Ctrl+Y (redo) each persist exactly once", async () => {
    const container = render();
    await pickFolder(container, "C:/work/design");
    makeOneEdit();
    invokeSpy.mockClear();

    pressCtrl("z");
    expect(saveCount()).toBe(1);

    invokeSpy.mockClear();
    pressCtrl("y"); // redo (W5: y without shift)
    expect(saveCount()).toBe(1);
  });

  it("W5: Ctrl+Shift+Y does NOT redo (shift gate on the y branch)", async () => {
    const container = render();
    await pickFolder(container, "C:/work/design");
    makeOneEdit();
    pressCtrl("z"); // move the edit onto the redo branch
    invokeSpy.mockClear();

    pressCtrl("y", /* shiftKey */ true); // must be ignored now
    expect(saveCount()).toBe(0);
  });
});
