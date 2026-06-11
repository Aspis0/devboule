// @vitest-environment jsdom
//
// Regression tests for the ModelPopover timeout slider persistence (MAJOR):
//   - a pointercancel mid-drag must persist like a pointerup
//   - a still-pending drag value must be persisted when the popover CLOSES / unmounts
//     without a pointerup ever firing (otherwise the dragged value is silently lost)

import { describe, it, expect, beforeEach, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { ModelPopover } from "./ModelPopover";
import type { DesignLlmBackend } from "../../../types/config";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const backend: DesignLlmBackend = { kind: "ollama", model: "qwen2.5-coder", timeoutSecs: 180 };

let container: HTMLElement;
let root: Root;
let onSave: ReturnType<typeof vi.fn>;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  onSave = vi.fn();
});

function mount(open: boolean) {
  act(() => {
    root = createRoot(container);
    root.render(
      createElement(ModelPopover, {
        open,
        onClose: () => {},
        backend,
        onSave,
        onOpenSettings: () => {},
      }),
    );
  });
}

function rerender(open: boolean) {
  act(() => {
    root.render(
      createElement(ModelPopover, {
        open,
        onClose: () => {},
        backend,
        onSave,
        onOpenSettings: () => {},
      }),
    );
  });
}

function slider(): HTMLInputElement {
  return container.querySelector('input[type="range"]') as HTMLInputElement;
}

/** Drag the slider to `value` WITHOUT firing a pointerup (the unsaved live state). */
function dragTo(value: number) {
  const input = slider();
  const setter = Object.getOwnPropertyDescriptor(
    window.HTMLInputElement.prototype,
    "value",
  )!.set!;
  act(() => {
    setter.call(input, String(value));
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

describe("ModelPopover — timeout slider persistence", () => {
  it("persists a pending value when the popover CLOSES without a pointerup", () => {
    mount(true);
    dragTo(300);
    // No pointerup fired yet — nothing saved.
    expect(onSave).not.toHaveBeenCalled();

    // Close the popover (open -> false): the slider unmounts; the pending value commits.
    rerender(false);
    expect(onSave).toHaveBeenCalledTimes(1);
    expect(onSave).toHaveBeenCalledWith(
      expect.objectContaining({ kind: "ollama", model: "qwen2.5-coder", timeoutSecs: 300 }),
    );
  });

  it("persists a pending value when the popover UNMOUNTS without a pointerup", () => {
    mount(true);
    dragTo(240);
    expect(onSave).not.toHaveBeenCalled();

    act(() => root.unmount());
    expect(onSave).toHaveBeenCalledTimes(1);
    expect(onSave).toHaveBeenCalledWith(
      expect.objectContaining({ timeoutSecs: 240 }),
    );
  });

  it("does NOT persist on close when the value is unchanged", () => {
    mount(true);
    // No drag — close immediately.
    rerender(false);
    expect(onSave).not.toHaveBeenCalled();
  });

  it("persists on pointercancel mid-drag (like pointerup)", () => {
    mount(true);
    const input = slider();
    const setter = Object.getOwnPropertyDescriptor(
      window.HTMLInputElement.prototype,
      "value",
    )!.set!;
    act(() => {
      setter.call(input, "270");
      input.dispatchEvent(new Event("input", { bubbles: true }));
      // jsdom lacks PointerEvent in some setups; a plain Event with the right type still
      // triggers React's onPointerCancel synthetic handler.
      input.dispatchEvent(new Event("pointercancel", { bubbles: true }));
    });
    expect(onSave).toHaveBeenCalledWith(
      expect.objectContaining({ timeoutSecs: 270 }),
    );
  });
});
