// @vitest-environment jsdom
//
// TopBar tests: the save-state dot reflects "saved"/"dirty"/"writing", the Save
// split button disables while saving / when no project, undo/redo disabled flags,
// and the path chip only renders once a project is open.

import { describe, it, expect, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { TopBar, type TopBarProps } from "./TopBar";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

function baseProps(over: Partial<TopBarProps> = {}): TopBarProps {
  return {
    projectName: "Landing",
    workingFolderPath: "/x/landing",
    projectOpen: true,
    saveState: "saved",
    saving: false,
    busy: false,
    canUndo: false,
    canRedo: false,
    onUndo: vi.fn(),
    onRedo: vi.fn(),
    fullscreen: false,
    onToggleFullscreen: vi.fn(),
    recent: [],
    renamingId: null,
    renameDraft: "",
    setRenameDraft: vi.fn(),
    beginRename: vi.fn(),
    commitRename: vi.fn(),
    cancelRename: vi.fn(),
    removeEntry: vi.fn(),
    openEntry: vi.fn(),
    onNewProject: vi.fn(),
    onOpenFolder: vi.fn(),
    oracleStatus: { grounded: false },
    tokens: {},
    invoke: (async () => undefined) as TopBarProps["invoke"],
    tauri: true,
    runExport: vi.fn(),
    exportTokens: vi.fn(),
    onConsolidate: vi.fn(),
    onHandoff: vi.fn(),
    onPreview: vi.fn(),
    previewing: false,
    ...over,
  };
}

function render(props: TopBarProps): HTMLElement {
  const container = document.createElement("div");
  document.body.appendChild(container);
  act(() => {
    createRoot(container).render(createElement(TopBar, props));
  });
  return container;
}

function status(container: HTMLElement): HTMLElement {
  return container.querySelector("[data-testid=tb-status]") as HTMLElement;
}

describe("TopBar — save-state dot", () => {
  it("renders 'Saved' with data-state=clean", () => {
    const c = render(baseProps({ saveState: "saved" }));
    expect(status(c).getAttribute("data-state")).toBe("clean");
    expect(status(c).textContent).toContain("Saved");
  });

  it("renders 'Unsaved changes' with data-state=dirty", () => {
    const c = render(baseProps({ saveState: "dirty" }));
    expect(status(c).getAttribute("data-state")).toBe("dirty");
    expect(status(c).textContent).toContain("Unsaved changes");
  });

  it("renders 'Saving…' with data-state=writing", () => {
    const c = render(baseProps({ saveState: "writing" }));
    expect(status(c).getAttribute("data-state")).toBe("writing");
    expect(status(c).textContent).toContain("Saving");
  });
});

describe("TopBar — controls", () => {
  it("disables the Save split button while saving", () => {
    const c = render(baseProps({ saving: true }));
    const saveBtn = Array.from(c.querySelectorAll("button")).find(
      (b) => b.textContent?.trim().startsWith("Save to repo"),
    ) as HTMLButtonElement;
    expect(saveBtn.disabled).toBe(true);
  });

  it("disables Save when no project is open", () => {
    const c = render(baseProps({ projectOpen: false, projectName: "" }));
    const saveBtn = Array.from(c.querySelectorAll("button")).find(
      (b) => b.textContent?.trim().startsWith("Save to repo"),
    ) as HTMLButtonElement;
    expect(saveBtn.disabled).toBe(true);
  });

  it("reflects canUndo/canRedo on the history buttons", () => {
    const c = render(baseProps({ canUndo: true, canRedo: false }));
    const undo = c.querySelector("[aria-label=Undo]") as HTMLButtonElement;
    const redo = c.querySelector("[aria-label=Redo]") as HTMLButtonElement;
    expect(undo.disabled).toBe(false);
    expect(redo.disabled).toBe(true);
  });

  it("renders the path chip only when a project is open", () => {
    expect(render(baseProps({ projectOpen: true })).querySelector(".tb-path")).toBeTruthy();
    expect(
      render(baseProps({ projectOpen: false })).querySelector(".tb-path"),
    ).toBeNull();
  });

  it("shows 'No project' when projectName is empty", () => {
    const c = render(baseProps({ projectName: "", projectOpen: false }));
    expect((c.querySelector(".tb-proj") as HTMLElement).textContent).toContain(
      "No project",
    );
  });

  it("toggles fullscreen via the focus button", () => {
    const onToggleFullscreen = vi.fn();
    const c = render(baseProps({ onToggleFullscreen }));
    const btn = c.querySelector(
      "[aria-label='Enter focus mode']",
    ) as HTMLButtonElement;
    act(() => btn.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(onToggleFullscreen).toHaveBeenCalledTimes(1);
  });
});
