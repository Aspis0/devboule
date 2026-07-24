// @vitest-environment jsdom
//
// Labs page: Design toggle (local) + Pigeon card (build-locked OFF in alpha) +
// "coming soon" cards (SkillOpt, ORPO Night). Pigeon must not be flipable from
// the UI in this build — backend also hard-rejects enable writes.

import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";

const invokeMock = vi.fn(async (command: string, args?: Record<string, unknown>) => {
  switch (command) {
    case "get_pigeon_enabled":
      return false;
    case "set_pigeon_enabled":
      return Boolean(args?.enabled);
    default:
      return undefined;
  }
});

vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (...args: unknown[]) =>
    invokeMock(...(args as [string, Record<string, unknown>?])),
}));

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let container: HTMLElement;

beforeEach(() => {
  invokeMock.mockClear();
  container = document.createElement("div");
  document.body.appendChild(container);
});

afterEach(() => {
  container.remove();
});

async function mount() {
  const { LabsView } = await import("./LabsView");
  await act(async () => {
    createRoot(container).render(createElement(LabsView));
  });
  // flush the chained get_* promise resolutions (then → finally) into state
  await act(async () => {
    for (let i = 0; i < 5; i += 1) await Promise.resolve();
  });
}

function switchByLabel(label: string): HTMLButtonElement {
  const el = container.querySelector<HTMLButtonElement>(
    `button[role="switch"][aria-label="${label}"]`,
  );
  expect(el).toBeTruthy();
  return el!;
}

describe("LabsView", () => {
  it("renders a Pigeon card", async () => {
    await mount();
    expect(container.textContent).toContain("Pigeon");
  });

  it("does NOT render an Oracle toggle (moved to Oracle page)", async () => {
    await mount();
    expect(container.querySelector('[aria-label="Toggle Oracle"]')).toBeNull();
  });

  it("renders the Pigeon toggle disabled (hard-off in this build)", async () => {
    await mount();
    const pigeon = switchByLabel("Toggle Pigeon");
    expect(pigeon.disabled).toBe(true);
    expect(pigeon.getAttribute("aria-checked")).toBe("false");
    expect(pigeon.getAttribute("aria-disabled")).toBe("true");
    expect(container.textContent).toContain("Disabled in this build");
    // Build-locked: no get/set IPC for Pigeon at all.
    expect(invokeMock).not.toHaveBeenCalledWith("get_pigeon_enabled");
    expect(invokeMock).not.toHaveBeenCalledWith(
      "set_pigeon_enabled",
      expect.anything(),
    );
  });

  it("does not call set_pigeon_enabled when the Pigeon switch is clicked", async () => {
    await mount();
    const pigeon = switchByLabel("Toggle Pigeon");
    await act(async () => {
      pigeon.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(invokeMock).not.toHaveBeenCalledWith(
      "set_pigeon_enabled",
      expect.anything(),
    );
    // Still off after the click attempt.
    expect(pigeon.getAttribute("aria-checked")).toBe("false");
    expect(pigeon.disabled).toBe(true);
  });
});
