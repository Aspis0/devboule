// @vitest-environment jsdom
//
// Tests for the native folder-picker UX in DesignView. The working folder must be
// CHOSEN via the OS directory dialog (@tauri-apps/plugin-dialog `open`), never
// typed. These tests verify:
//   - picking a folder sets the read-only path display and enables Create/Load,
//   - cancelling the dialog (open -> null) is a silent no-op,
//   - the chosen absolute path is exactly what reaches design_create_project /
//     design_load_project.
// Both the backend (`invokeBackendCommand`) and the dialog `open` are mocked.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
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

// ---- native folder picker mock --------------------------------------------
const dialogCtl: { nextPick: string | null } = { nextPick: null };
const openSpy = vi.fn(
  async (_opts?: Record<string, unknown>) => dialogCtl.nextPick,
);
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: (opts?: Record<string, unknown>) => openSpy(opts),
}));

// ---- stub the streaming hook + Canvas (not under test here) ----------------
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

vi.mock("./canvas/DesignCanvas", () => ({
  DesignCanvas: () => createElement("div", { "data-testid": "canvas" }),
}));

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let DesignView: typeof import("./DesignView").DesignView;

beforeEach(async () => {
  invokeSpy.mockClear();
  invokeSpy.mockImplementation(async () => undefined);
  openSpy.mockClear();
  dialogCtl.nextPick = null;
  ({ DesignView } = await import("./DesignView"));
});

function render(): HTMLElement {
  const container = document.createElement("div");
  document.body.appendChild(container);
  act(() => {
    createRoot(container).render(createElement(DesignView));
  });
  return container;
}

function findButton(container: HTMLElement, label: string): HTMLButtonElement {
  return Array.from(container.querySelectorAll("button")).find(
    (b) => b.textContent?.trim().startsWith(label),
  ) as HTMLButtonElement;
}

/** The working-folder path chip in the topbar (`.tb-path`); only present once a
 *  project is open. */
function pathChip(container: HTMLElement): HTMLElement | null {
  return container.querySelector(".tb-path");
}

function openProjectPopover(container: HTMLElement) {
  if (container.querySelector(".pop.left")) return;
  const proj = container.querySelector(".tb-proj") as HTMLButtonElement;
  act(() => proj.dispatchEvent(new MouseEvent("click", { bubbles: true })));
}

/** Open the ProjectPopover and click a footer action row ("New project" / "Open
 *  working folder"), which picks the folder ITSELF (dialog mocked to `pick`) and
 *  runs create / load. */
async function clickPopoverAction(
  container: HTMLElement,
  label: string,
  pick: string | null,
) {
  dialogCtl.nextPick = pick;
  openProjectPopover(container);
  const btn = findButton(container, label);
  await act(async () => {
    btn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

describe("DesignView — native folder picker", () => {
  it("has NO editable text input for the working folder path", () => {
    const container = render();
    // The folder must be CHOSEN, not typed. The temp Generate textarea and the
    // (disabled) edit-instruction input may exist, but none is a folder field.
    const textInputs = container.querySelectorAll("input[type=text]");
    textInputs.forEach((el) => {
      expect((el as HTMLInputElement).placeholder ?? "").not.toMatch(/folder/i);
    });
    // No path chip before a project is open.
    expect(pathChip(container)).toBeNull();
  });

  it("opens the native directory dialog with directory:true and a title", async () => {
    const container = render();
    await clickPopoverAction(container, "Open working folder", "C:/picked/dir");
    expect(openSpy).toHaveBeenCalledTimes(1);
    const opts = openSpy.mock.calls[0][0]!;
    expect(opts.directory).toBe(true);
    expect(opts.multiple).toBe(false);
    expect(typeof opts.title).toBe("string");
  });

  it("shows the chosen path chip after opening a working folder", async () => {
    const loaded: DesignProject = {
      meta: {
        schemaVersion: 1,
        id: "p",
        name: "Loaded",
        createdAt: "1970-01-01T00:00:00Z",
        updatedAt: "1970-01-01T00:00:00Z",
        canvas: { w: 1440, h: 1024, grid: 8 },
        nodeOrder: [],
      },
      manifest: { schemaVersion: 1, nodes: {} },
      components: {},
    };
    invokeSpy.mockImplementation(async (command: string) => {
      if (command === "design_load_project") return loaded;
      return undefined;
    });

    const container = render();
    // Before opening: no path chip, project name reads "No project".
    expect(pathChip(container)).toBeNull();
    expect(
      (container.querySelector(".tb-proj") as HTMLElement).textContent,
    ).toContain("No project");

    await clickPopoverAction(
      container,
      "Open working folder",
      "C:/target/.aspis-design/landing",
    );

    expect(pathChip(container)?.textContent).toContain(
      "C:/target/.aspis-design/landing",
    );
  });

  it("treats a cancelled dialog (open -> null) as a no-op", async () => {
    const container = render();
    await clickPopoverAction(container, "Open working folder", null);

    // No path chip set, no error surfaced.
    expect(pathChip(container)).toBeNull();
    expect(container.textContent).not.toMatch(/Choose a working folder/i);
  });

  it("passes the EXACT chosen path to design_create_project", async () => {
    const created: DesignProject = {
      meta: {
        schemaVersion: 1,
        id: "demo",
        name: "Demo landing",
        createdAt: "1970-01-01T00:00:00Z",
        updatedAt: "1970-01-01T00:00:00Z",
        canvas: { w: 1440, h: 1024, grid: 8 },
        nodeOrder: [],
      },
      manifest: { schemaVersion: 1, nodes: {} },
      components: {},
    };
    invokeSpy.mockImplementation(async (command: string) => {
      if (command === "design_create_project") return created;
      return undefined;
    });

    const container = render();
    await clickPopoverAction(container, "New project", "D:/projects/site");

    const call = invokeSpy.mock.calls.find(
      (c) => c[0] === "design_create_project",
    );
    expect(call).toBeTruthy();
    expect((call![1] as { workingFolderPath: string }).workingFolderPath).toBe(
      "D:/projects/site",
    );
  });

  it("passes the EXACT chosen path to design_load_project", async () => {
    const loaded: DesignProject = {
      meta: {
        schemaVersion: 1,
        id: "p",
        name: "Loaded",
        createdAt: "1970-01-01T00:00:00Z",
        updatedAt: "1970-01-01T00:00:00Z",
        canvas: { w: 1440, h: 1024, grid: 8 },
        nodeOrder: [],
      },
      manifest: { schemaVersion: 1, nodes: {} },
      components: {},
    };
    invokeSpy.mockImplementation(async (command: string) => {
      if (command === "design_load_project") return loaded;
      return undefined;
    });

    const container = render();
    await clickPopoverAction(
      container,
      "Open working folder",
      "/Users/me/work/landing",
    );

    const call = invokeSpy.mock.calls.find((c) => c[0] === "design_load_project");
    expect(call).toBeTruthy();
    expect((call![1] as { workingFolderPath: string }).workingFolderPath).toBe(
      "/Users/me/work/landing",
    );
  });
});
