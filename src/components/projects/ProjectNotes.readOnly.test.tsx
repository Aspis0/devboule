// @vitest-environment jsdom
import { describe, expect, it, beforeEach } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";

import type { ProjectNote } from "../../types/backend";
import { ProjectNotes } from "./ProjectNotes";

const noop = () => undefined;

function notes(): ProjectNote[] {
  return [
    {
      id: "n1",
      text: "First note",
      source: "user",
      createdAt: "2026-06-01T00:00:00Z",
    } as unknown as ProjectNote,
  ];
}

let container: HTMLDivElement;
let root: Root;

function mount(readOnly: boolean) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => {
    root.render(
      createElement(ProjectNotes, {
        notes: notes(),
        noteDraft: "draft text",
        onNoteDraftChange: noop,
        onAppend: noop,
        isBusy: false,
        readOnly,
        revision: "rev-123456789012",
        modifiedAt: "2026-06-01T00:00:00Z",
        updatedAt: "2026-06-01T00:00:00Z",
      }),
    );
  });
  // CollapsibleSection starts collapsed — expand it so the draft controls render.
  const header = container.querySelector("button") as HTMLButtonElement;
  act(() => header.click());
}

function unmount() {
  act(() => root.unmount());
  container.remove();
}

describe("ProjectNotes read-only (archived) gating", () => {
  beforeEach(() => {
    (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
  });

  it("disables the draft textarea + Append button when readOnly", () => {
    mount(true);
    const textarea = container.querySelector("textarea") as HTMLTextAreaElement;
    const append = [...container.querySelectorAll("button")].find(
      (b) => (b.textContent ?? "").includes("Append"),
    ) as HTMLButtonElement;
    // Existing notes stay visible (read access intact).
    expect(container.textContent).toContain("First note");
    expect(textarea.disabled).toBe(true);
    expect(append.disabled).toBe(true);
    expect(textarea.getAttribute("placeholder")).toContain(
      "Archived project is read-only.",
    );
    unmount();
  });

  it("keeps the textarea editable when NOT readOnly (default behavior)", () => {
    mount(false);
    const textarea = container.querySelector("textarea") as HTMLTextAreaElement;
    const append = [...container.querySelectorAll("button")].find(
      (b) => (b.textContent ?? "").includes("Append"),
    ) as HTMLButtonElement;
    expect(container.textContent).toContain("First note");
    expect(textarea.disabled).toBe(false);
    // With a non-empty draft and isBusy=false, Append is enabled.
    expect(append.disabled).toBe(false);
    expect(textarea.getAttribute("placeholder")).toContain(
      "Append a project note",
    );
    unmount();
  });
});
