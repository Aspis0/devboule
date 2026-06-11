// @vitest-environment jsdom
//
// ProjectPopover tests: renders the registry list with a Check on the open project,
// drives the inline-rename flow (the SAME aria-labels the recent registry test
// relies on), and the footer New/Open actions.

import { describe, it, expect, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import type { DesignProjectEntry } from "../../../types/design";
import { ProjectPopover, type ProjectPopoverProps } from "./ProjectPopover";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

function entry(over: Partial<DesignProjectEntry>): DesignProjectEntry {
  return {
    id: "id1",
    name: "Landing",
    workingFolderPath: "/x/landing",
    createdAt: "",
    updatedAt: "",
    lastOpenedAt: "",
    ...over,
  };
}

function baseProps(over: Partial<ProjectPopoverProps> = {}): ProjectPopoverProps {
  return {
    open: true,
    onClose: vi.fn(),
    recent: [
      entry({ id: "a", name: "Alpha", workingFolderPath: "/x/alpha" }),
      entry({ id: "b", name: "Beta", workingFolderPath: "/x/beta" }),
    ],
    currentFolder: "/x/alpha",
    busy: false,
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
    ...over,
  };
}

function render(props: ProjectPopoverProps): HTMLElement {
  const container = document.createElement("div");
  document.body.appendChild(container);
  act(() => {
    createRoot(container).render(createElement(ProjectPopover, props));
  });
  return container;
}

function items(container: HTMLElement): HTMLElement[] {
  return Array.from(
    container.querySelectorAll("[data-testid=design-recent-item]"),
  ) as HTMLElement[];
}

describe("ProjectPopover", () => {
  it("does not render when closed", () => {
    const c = render(baseProps({ open: false }));
    expect(c.querySelector(".pop.left")).toBeNull();
  });

  it("lists every registry entry", () => {
    const c = render(baseProps());
    expect(items(c).length).toBe(2);
    expect(c.textContent).toContain("Alpha");
    expect(c.textContent).toContain("/x/beta");
  });

  it("marks the OPEN project with a check (sel row)", () => {
    const c = render(baseProps({ currentFolder: "/x/alpha" }));
    const sel = c.querySelectorAll(".pop-row.sel");
    expect(sel.length).toBe(1);
    expect((sel[0] as HTMLElement).textContent).toContain("Alpha");
  });

  it("opens an entry's working folder on click", () => {
    const openEntry = vi.fn();
    const c = render(baseProps({ openEntry, currentFolder: "" }));
    const openBtn = items(c)[0].querySelector("button") as HTMLButtonElement;
    act(() => openBtn.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(openEntry).toHaveBeenCalledWith(
      expect.objectContaining({ id: "a", workingFolderPath: "/x/alpha" }),
    );
  });

  it("begins rename from the per-row pencil", () => {
    const beginRename = vi.fn();
    const c = render(baseProps({ beginRename, currentFolder: "" }));
    const pencil = c.querySelector(
      "[aria-label='Rename Alpha']",
    ) as HTMLButtonElement;
    act(() => pencil.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(beginRename).toHaveBeenCalledWith(
      expect.objectContaining({ id: "a" }),
    );
  });

  it("commits rename via the Save name control (Enter and button)", () => {
    const commitRename = vi.fn();
    const c = render(
      baseProps({ renamingId: "a", renameDraft: "Renamed", commitRename }),
    );
    const input = c.querySelector(
      "input[aria-label='Rename project']",
    ) as HTMLInputElement;
    expect(input).toBeTruthy();
    act(() =>
      input.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
      ),
    );
    const save = c.querySelector("[aria-label='Save name']") as HTMLButtonElement;
    act(() => save.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(commitRename).toHaveBeenCalledWith("a");
    expect(commitRename).toHaveBeenCalledTimes(2); // Enter + button
  });

  it("removes (unregisters) an entry", () => {
    const removeEntry = vi.fn();
    const c = render(baseProps({ removeEntry, currentFolder: "" }));
    const rm = c.querySelector(
      "[aria-label='Remove Beta from the list']",
    ) as HTMLButtonElement;
    act(() => rm.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(removeEntry).toHaveBeenCalledWith("b");
  });

  it("runs the New project + Open working folder footer actions and closes", () => {
    const onNewProject = vi.fn();
    const onOpenFolder = vi.fn();
    const onClose = vi.fn();
    const c = render(baseProps({ onNewProject, onOpenFolder, onClose }));
    const newBtn = Array.from(c.querySelectorAll("button")).find((b) =>
      b.textContent?.trim().startsWith("New project"),
    ) as HTMLButtonElement;
    const openBtn = Array.from(c.querySelectorAll("button")).find((b) =>
      b.textContent?.trim().startsWith("Open working folder"),
    ) as HTMLButtonElement;
    act(() => newBtn.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    act(() => openBtn.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(onNewProject).toHaveBeenCalledTimes(1);
    expect(onOpenFolder).toHaveBeenCalledTimes(1);
    expect(onClose).toHaveBeenCalled();
  });

  it("uses a thumbnail image when present, else a color block", () => {
    const c = render(
      baseProps({
        recent: [
          entry({ id: "a", thumbnailPath: "file:///t.png" }),
          entry({ id: "b" }),
        ],
        currentFolder: "",
      }),
    );
    const thumbs = c.querySelectorAll(".thumb");
    expect(thumbs[0].tagName).toBe("IMG");
    expect((thumbs[0] as HTMLImageElement).getAttribute("src")).toBe(
      "file:///t.png",
    );
    expect(thumbs[1].tagName).toBe("DIV");
  });
});
