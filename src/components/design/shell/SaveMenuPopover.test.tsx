// @vitest-environment jsdom
//
// SaveMenuPopover tests: "Save to repo" runs onSave + closes; the "Save & hand off"
// agents row is now ENABLED (Phase D) and opens the hand-off modal via onHandoff.

import { describe, it, expect, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { SaveMenuPopover, type SaveMenuPopoverProps } from "./SaveMenuPopover";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

function render(props: SaveMenuPopoverProps): HTMLElement {
  const container = document.createElement("div");
  document.body.appendChild(container);
  act(() => {
    createRoot(container).render(createElement(SaveMenuPopover, props));
  });
  return container;
}

describe("SaveMenuPopover", () => {
  it("Save to repo runs onSave and closes", () => {
    const onSave = vi.fn();
    const onClose = vi.fn();
    const c = render({
      open: true,
      onClose,
      disabled: false,
      onSave,
      onHandoff: vi.fn(),
    });
    const save = Array.from(c.querySelectorAll("button")).find((b) =>
      b.textContent?.trim().startsWith("Save to repo"),
    ) as HTMLButtonElement;
    act(() => save.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(onSave).toHaveBeenCalledTimes(1);
    expect(onClose).toHaveBeenCalled();
  });

  it("the hand-off (agents) row is enabled and opens the modal (keeps the NEW badge)", () => {
    const onHandoff = vi.fn();
    const onClose = vi.fn();
    const c = render({
      open: true,
      onClose,
      disabled: false,
      onSave: vi.fn(),
      onHandoff,
    });
    const agents = c.querySelector(".pop-row.agents") as HTMLButtonElement;
    expect(agents).toBeTruthy();
    expect(agents.disabled).toBe(false);
    expect(c.querySelector(".new-badge")?.textContent).toBe("NEW");
    act(() => agents.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(onHandoff).toHaveBeenCalledTimes(1);
    expect(onClose).toHaveBeenCalled();
  });

  it("disables BOTH rows when disabled (no project open / save in flight)", () => {
    const onHandoff = vi.fn();
    const c = render({
      open: true,
      onClose: vi.fn(),
      disabled: true,
      onSave: vi.fn(),
      onHandoff,
    });
    const save = Array.from(c.querySelectorAll("button")).find((b) =>
      b.textContent?.trim().startsWith("Save to repo"),
    ) as HTMLButtonElement;
    expect(save.disabled).toBe(true);
    const agents = c.querySelector(".pop-row.agents") as HTMLButtonElement;
    expect(agents.disabled).toBe(true);
  });
});
