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

vi.mock("./Canvas", () => ({
  Canvas: () => createElement("div", { "data-testid": "canvas" }),
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

function pathDisplay(container: HTMLElement): HTMLElement {
  return container.querySelector(
    "[data-testid=design-folder-path]",
  ) as HTMLElement;
}

/** Click "Choose folder…" with the dialog mocked to return `pick`. */
async function clickPick(container: HTMLElement, pick: string | null) {
  dialogCtl.nextPick = pick;
  const btn = findButton(container, "Choose folder");
  await act(async () => {
    btn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

describe("DesignView — native folder picker", () => {
  it("has NO editable text input for the working folder path", () => {
    const container = render();
    // The folder must be chosen, not typed. The Generate textarea and the (disabled)
    // edit-instruction input may exist, but there is no path text field to type into.
    const textInputs = container.querySelectorAll("input[type=text]");
    // Only the edit-instruction input is a text input; assert none of them is the
    // folder field by confirming the read-only display exists instead.
    expect(pathDisplay(container)).toBeTruthy();
    // The edit input is disabled until a node is selected; it is not the folder path.
    textInputs.forEach((el) => {
      expect((el as HTMLInputElement).placeholder).not.toMatch(/folder/i);
    });
  });

  it("opens the native directory dialog with directory:true and a title", async () => {
    const container = render();
    await clickPick(container, "C:/picked/dir");
    expect(openSpy).toHaveBeenCalledTimes(1);
    const opts = openSpy.mock.calls[0][0]!;
    expect(opts.directory).toBe(true);
    expect(opts.multiple).toBe(false);
    expect(typeof opts.title).toBe("string");
  });

  it("shows the chosen path and enables Create/Load after picking", async () => {
    const container = render();

    // Before picking: no path shown, Create/Load disabled.
    expect(pathDisplay(container).textContent).toContain("No folder chosen");
    expect(findButton(container, "Create").disabled).toBe(true);
    expect(findButton(container, "Load").disabled).toBe(true);

    await clickPick(container, "C:/target/.aspis-design/landing");

    expect(pathDisplay(container).textContent).toContain(
      "C:/target/.aspis-design/landing",
    );
    expect(findButton(container, "Create").disabled).toBe(false);
    expect(findButton(container, "Load").disabled).toBe(false);
  });

  it("treats a cancelled dialog (open -> null) as a no-op", async () => {
    const container = render();
    await clickPick(container, null);

    // No path set, controls stay disabled, no error surfaced.
    expect(pathDisplay(container).textContent).toContain("No folder chosen");
    expect(findButton(container, "Create").disabled).toBe(true);
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
    await clickPick(container, "D:/projects/site");

    const createBtn = findButton(container, "Create");
    await act(async () => {
      createBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

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
    await clickPick(container, "/Users/me/work/landing");

    const loadBtn = findButton(container, "Load");
    await act(async () => {
      loadBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    const call = invokeSpy.mock.calls.find((c) => c[0] === "design_load_project");
    expect(call).toBeTruthy();
    expect((call![1] as { workingFolderPath: string }).workingFolderPath).toBe(
      "/Users/me/work/landing",
    );
  });

  it("clears the chosen path with the clear button and re-disables Create/Load", async () => {
    const container = render();
    await clickPick(container, "C:/some/dir");
    expect(findButton(container, "Create").disabled).toBe(false);

    const clearBtn = container.querySelector(
      "[aria-label='Clear chosen folder']",
    ) as HTMLButtonElement;
    expect(clearBtn).toBeTruthy();
    act(() => clearBtn.dispatchEvent(new MouseEvent("click", { bubbles: true })));

    expect(pathDisplay(container).textContent).toContain("No folder chosen");
    expect(findButton(container, "Create").disabled).toBe(true);
    expect(findButton(container, "Load").disabled).toBe(true);
  });
});
