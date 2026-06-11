// @vitest-environment jsdom
//
// Phase 3 (management plane) tests for the recent-projects registry surface in
// DesignView. The registry is metadata-only (config.json on the Rust side); here we
// mock `invokeBackendCommand` to assert the UX:
//   - on mount the list is fetched via design_registry_list and rendered,
//   - clicking a recent entry loads it via design_load_project (and remembers it),
//   - design_registry_remember is called after a successful Create and Load,
//   - a missing-folder load surfaces a graceful status (no crash) + a prune hint,
//   - Remove (unregister) calls design_registry_remove with removeFiles:false and
//     updates the list from the command's returned array.
// Canvas + the streaming hook are stubbed (not under test).

import { describe, it, expect, beforeEach, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import type { DesignProject, DesignProjectEntry } from "../../types/design";

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
const openSpy = vi.fn(async () => dialogCtl.nextPick);
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: () => openSpy(),
}));

// ---- stub the streaming hook + Canvas (not under test) --------------------
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

function entry(over: Partial<DesignProjectEntry>): DesignProjectEntry {
  return {
    id: "id1",
    name: "Landing",
    workingFolderPath: "/target/.aspis-design/landing",
    createdAt: "2021-01-01T00:00:00Z",
    updatedAt: "2021-01-01T00:00:00Z",
    lastOpenedAt: "2021-01-01T00:00:00Z",
    ...over,
  };
}

function loadedProject(name: string): DesignProject {
  return {
    meta: {
      schemaVersion: 1,
      id: "p",
      name,
      createdAt: "1970-01-01T00:00:00Z",
      updatedAt: "1970-01-01T00:00:00Z",
      canvas: { w: 1440, h: 1024, grid: 8 },
      nodeOrder: [],
    },
    manifest: { schemaVersion: 1, nodes: {} },
    components: {},
  };
}

/** Default mock: list returns whatever `listCtl.value` holds; other commands no-op. */
const listCtl: { value: DesignProjectEntry[] } = { value: [] };

beforeEach(async () => {
  invokeSpy.mockReset();
  openSpy.mockClear();
  dialogCtl.nextPick = null;
  listCtl.value = [];
  invokeSpy.mockImplementation(async (command: string) => {
    if (command === "design_registry_list") return listCtl.value;
    return undefined;
  });
  ({ DesignView } = await import("./DesignView"));
});

async function flush() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

function render(): HTMLElement {
  const container = document.createElement("div");
  document.body.appendChild(container);
  act(() => {
    createRoot(container).render(createElement(DesignView));
  });
  return container;
}

function recentItems(container: HTMLElement): HTMLElement[] {
  return Array.from(
    container.querySelectorAll("[data-testid=design-recent-item]"),
  ) as HTMLElement[];
}

function findButton(container: HTMLElement, label: string): HTMLButtonElement {
  return Array.from(container.querySelectorAll("button")).find((b) =>
    b.textContent?.trim().startsWith(label),
  ) as HTMLButtonElement;
}

// The registry list + folder actions now live inside the TopBar's ProjectPopover.
// Ensure it is OPEN (idempotent: the trigger toggles, so only click when closed) so
// its rows are mounted in the DOM. The left-variant `.pop.left` panel presence is
// the open signal.
function openProjectPopover(container: HTMLElement) {
  if (container.querySelector(".pop.left")) return; // already open
  const proj = container.querySelector(".tb-proj") as HTMLButtonElement;
  act(() => proj.dispatchEvent(new MouseEvent("click", { bubbles: true })));
}

// "Create" maps to the popover's "New project…" row, which picks a folder ITSELF
// (the dialog mock returns `pick`) and then creates it. "Load" maps to "Open
// working folder…". A single click does both pick + action.
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

describe("DesignView — recent-projects registry", () => {
  it("fetches and renders the recent list on mount", async () => {
    listCtl.value = [
      entry({ id: "a", name: "Alpha", workingFolderPath: "/x/alpha" }),
      entry({ id: "b", name: "Beta", workingFolderPath: "/x/beta" }),
    ];
    const container = render();
    await flush();
    openProjectPopover(container);

    expect(
      invokeSpy.mock.calls.some((c) => c[0] === "design_registry_list"),
    ).toBe(true);
    const items = recentItems(container);
    expect(items.length).toBe(2);
    expect(container.textContent).toContain("Alpha");
    expect(container.textContent).toContain("/x/alpha");
  });

  it("renders no recent rows when the registry is empty", async () => {
    listCtl.value = [];
    const container = render();
    await flush();
    openProjectPopover(container);
    // The popover's recent-list container mounts but holds no entry rows.
    expect(recentItems(container).length).toBe(0);
  });

  it("clicking a recent entry loads it via design_load_project", async () => {
    listCtl.value = [entry({ id: "a", name: "Alpha", workingFolderPath: "/x/alpha" })];
    invokeSpy.mockImplementation(async (command: string) => {
      if (command === "design_registry_list") return listCtl.value;
      if (command === "design_load_project") return loadedProject("Alpha");
      if (command === "design_registry_remember") return listCtl.value;
      return undefined;
    });
    const container = render();
    await flush();
    openProjectPopover(container);

    const openBtn = recentItems(container)[0].querySelector(
      "button",
    ) as HTMLButtonElement;
    await act(async () => {
      openBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    const loadCall = invokeSpy.mock.calls.find(
      (c) => c[0] === "design_load_project",
    );
    expect(loadCall).toBeTruthy();
    expect((loadCall![1] as { workingFolderPath: string }).workingFolderPath).toBe(
      "/x/alpha",
    );
  });

  it("calls design_registry_remember after a successful Create", async () => {
    const created = loadedProject("Demo landing");
    invokeSpy.mockImplementation(async (command: string) => {
      if (command === "design_registry_list") return [];
      if (command === "design_create_project") return created;
      if (command === "design_registry_remember") return [];
      return undefined;
    });
    const container = render();
    await flush();
    // "New project…" picks D:/projects/site and creates it in one click.
    await clickPopoverAction(container, "New project", "D:/projects/site");

    const rememberCall = invokeSpy.mock.calls.find(
      (c) => c[0] === "design_registry_remember",
    );
    expect(rememberCall).toBeTruthy();
    const arg = rememberCall![1] as { entry: DesignProjectEntry };
    expect(arg.entry.workingFolderPath).toBe("D:/projects/site");
    expect(arg.entry.name).toBe("Demo landing");
  });

  it("calls design_registry_remember after a successful Load (picker)", async () => {
    invokeSpy.mockImplementation(async (command: string) => {
      if (command === "design_registry_list") return [];
      if (command === "design_load_project") return loadedProject("Loaded");
      if (command === "design_registry_remember") return [];
      return undefined;
    });
    const container = render();
    await flush();
    // "Open working folder…" picks the path and loads it in one click.
    await clickPopoverAction(container, "Open working folder", "/Users/me/work/landing");

    const rememberCall = invokeSpy.mock.calls.find(
      (c) => c[0] === "design_registry_remember",
    );
    expect(rememberCall).toBeTruthy();
    expect(
      (rememberCall![1] as { entry: DesignProjectEntry }).entry.workingFolderPath,
    ).toBe("/Users/me/work/landing");
  });

  it("shows a graceful status (no crash) when a recent folder is missing", async () => {
    listCtl.value = [entry({ id: "a", name: "Gone", workingFolderPath: "/x/gone" })];
    invokeSpy.mockImplementation(async (command: string) => {
      if (command === "design_registry_list") return listCtl.value;
      if (command === "design_load_project")
        throw new Error("working folder does not exist or is unreadable");
      return undefined;
    });
    const container = render();
    await flush();
    openProjectPopover(container);

    const openBtn = recentItems(container)[0].querySelector(
      "button",
    ) as HTMLButtonElement;
    await act(async () => {
      openBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    // Error surfaced with a prune hint; the recent entry is still listed (no crash).
    // The error lives in the assistant status column (always mounted), the recent row
    // re-opens via the popover.
    expect(container.textContent).toMatch(/working folder does not exist/i);
    expect(container.textContent).toMatch(/Remove on the entry/i);
    openProjectPopover(container);
    expect(recentItems(container).length).toBe(1);
  });

  it("Remove unregisters (removeFiles:false) and adopts the returned list", async () => {
    listCtl.value = [
      entry({ id: "a", name: "Alpha", workingFolderPath: "/x/alpha" }),
      entry({ id: "b", name: "Beta", workingFolderPath: "/x/beta" }),
    ];
    invokeSpy.mockImplementation(async (command: string, args) => {
      if (command === "design_registry_list") return listCtl.value;
      if (command === "design_registry_remove") {
        const id = (args as { args: { id: string } }).args.id;
        return listCtl.value.filter((e) => e.id !== id);
      }
      return undefined;
    });
    const container = render();
    await flush();
    openProjectPopover(container);
    expect(recentItems(container).length).toBe(2);

    const removeBtn = container.querySelector(
      "[aria-label='Remove Alpha from the list']",
    ) as HTMLButtonElement;
    await act(async () => {
      removeBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    const removeCall = invokeSpy.mock.calls.find(
      (c) => c[0] === "design_registry_remove",
    );
    expect(removeCall).toBeTruthy();
    const removeArgs = (removeCall![1] as { args: { id: string; removeFiles: boolean } })
      .args;
    expect(removeArgs.id).toBe("a");
    expect(removeArgs.removeFiles).toBe(false);
    // The list now reflects the command's returned array.
    expect(recentItems(container).length).toBe(1);
    expect(container.textContent).toContain("Beta");
    expect(container.textContent).not.toContain("Alpha");
  });

  it("Rename calls design_registry_rename with the new name", async () => {
    listCtl.value = [entry({ id: "a", name: "Alpha", workingFolderPath: "/x/alpha" })];
    invokeSpy.mockImplementation(async (command: string, args) => {
      if (command === "design_registry_list") return listCtl.value;
      if (command === "design_registry_rename") {
        const name = (args as { name: string }).name;
        return [entry({ id: "a", name, workingFolderPath: "/x/alpha" })];
      }
      return undefined;
    });
    const container = render();
    await flush();
    openProjectPopover(container);

    const renameBtn = container.querySelector(
      "[aria-label='Rename Alpha']",
    ) as HTMLButtonElement;
    act(() => renameBtn.dispatchEvent(new MouseEvent("click", { bubbles: true })));

    const input = container.querySelector(
      "input[aria-label='Rename project']",
    ) as HTMLInputElement;
    expect(input).toBeTruthy();
    act(() => {
      // React tracks the input's value via a private setter; bypass it so the
      // dispatched `input` event reaches the controlled onChange.
      const setter = Object.getOwnPropertyDescriptor(
        window.HTMLInputElement.prototype,
        "value",
      )!.set!;
      setter.call(input, "Renamed");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
    const saveBtn = container.querySelector(
      "[aria-label='Save name']",
    ) as HTMLButtonElement;
    await act(async () => {
      saveBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    const renameCall = invokeSpy.mock.calls.find(
      (c) => c[0] === "design_registry_rename",
    );
    expect(renameCall).toBeTruthy();
    expect((renameCall![1] as { id: string; name: string }).id).toBe("a");
    expect((renameCall![1] as { id: string; name: string }).name).toBe("Renamed");
    expect(container.textContent).toContain("Renamed");
  });
});
