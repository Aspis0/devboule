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

// BUG 3: switching to a local provider (ollama/oMLX/CLI) that has no saved config used to be a
// SILENT no-op — the row looked unclickable. Clicking it must give visible feedback, not nothing.
// The failed kind must NOT appear as the active selection (only the saved kind is "sel").
describe("ModelPopover — provider switch feedback (bug #3)", () => {
  const claudeBackend: DesignLlmBackend = { kind: "claude", timeoutSecs: 180 };

  function mountWith(b: DesignLlmBackend | null, openSettings = () => {}) {
    act(() => {
      root = createRoot(container);
      root.render(
        createElement(ModelPopover, {
          open: true,
          onClose: () => {},
          backend: b,
          onSave,
          onOpenSettings: openSettings,
        }),
      );
    });
  }

  function clickProvider(name: string) {
    const btn = [...container.querySelectorAll("button.mp-row")].find((b) =>
      b.textContent?.includes(name),
    ) as HTMLButtonElement;
    if (!btn) throw new Error(`provider row "${name}" not found`);
    act(() => {
      btn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
  }

  function providerRow(name: string): HTMLButtonElement {
    const btn = [...container.querySelectorAll("button.mp-row")].find((b) =>
      b.textContent?.includes(name),
    ) as HTMLButtonElement;
    if (!btn) throw new Error(`provider row "${name}" not found`);
    return btn;
  }

  it("labels the popover as the global Design model (same as Settings)", () => {
    mountWith(claudeBackend);
    expect(container.querySelector('[data-testid="design-model-global-label"]')?.textContent).toMatch(
      /DESIGN MODEL \(GLOBAL\)/i,
    );
    expect(container.querySelector('[data-testid="design-model-global-note"]')?.textContent).toMatch(
      /Also editable in Settings/i,
    );
  });

  it("clicking an unconfigured local provider shows a dedicated not-saved hint (not a silent no-op)", () => {
    mountWith(claudeBackend);
    // No dedicated hint before any failed click (the provider list + Settings button are always
    // present, so we assert on a DEDICATED element, not on raw text — that would be tautological).
    expect(container.querySelector('[data-testid="provider-config-hint"]')).toBeNull();
    clickProvider("Ollama");
    // It must NOT silently switch to a backend with no model…
    expect(onSave).not.toHaveBeenCalled();
    // …and a dedicated hint must appear, naming the clicked provider + saying Not saved.
    const hint = container.querySelector('[data-testid="provider-config-hint"]');
    expect(hint, "expected a provider-config-hint after clicking an unconfigured provider").not.toBeNull();
    expect(hint?.textContent ?? "").toMatch(/Not saved/i);
    expect(hint?.textContent ?? "").toMatch(/Ollama/);
    // Saved kind stays active; attempted kind must NOT look selected.
    expect(providerRow("Claude").classList.contains("sel")).toBe(true);
    expect(providerRow("Ollama").classList.contains("sel")).toBe(false);
    expect(providerRow("Ollama").getAttribute("data-needs-setup")).toBe("true");
  });

  it("Open Settings from the not-saved hint navigates to Settings", () => {
    const onOpenSettings = vi.fn();
    mountWith(claudeBackend, onOpenSettings);
    clickProvider("Ollama");
    const openBtn = container.querySelector(
      '[data-testid="provider-open-settings"]',
    ) as HTMLButtonElement;
    expect(openBtn).not.toBeNull();
    act(() => {
      openBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(onOpenSettings).toHaveBeenCalledTimes(1);
  });

  it("clicking a valid provider (no required fields) switches immediately", () => {
    mountWith(claudeBackend);
    clickProvider("Codex");
    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({ kind: "codex" }));
  });
});

// Pure helper: can this kind be saved from the popover given the current saved backend?
describe("nextBackendForKind — can-save decision", () => {
  it("returns valid when the kind needs no extra fields", async () => {
    const { nextBackendForKind } = await import("./ModelPopover");
    const r = nextBackendForKind("codex", { kind: "claude" });
    expect(r.valid).toBe(true);
    expect(r.value).toMatchObject({ kind: "codex" });
  });

  it("returns invalid when required fields are missing for the target kind", async () => {
    const { nextBackendForKind } = await import("./ModelPopover");
    const r = nextBackendForKind("ollama", { kind: "claude" });
    expect(r.valid).toBe(false);
    expect(r.value).toBeNull();
  });

  it("preserves same-kind fields when switching within a configured kind", async () => {
    const { nextBackendForKind } = await import("./ModelPopover");
    // Re-picking ollama with a saved model should stay valid and keep the model.
    const r = nextBackendForKind("ollama", {
      kind: "ollama",
      model: "qwen2.5-coder",
      timeoutSecs: 180,
    });
    expect(r.valid).toBe(true);
    expect(r.value).toMatchObject({ kind: "ollama", model: "qwen2.5-coder" });
  });
});
