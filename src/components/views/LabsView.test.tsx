// @vitest-environment jsdom
//
// Labs page: two experimental-feature cards (Pigeon, Oracle), each with an
// on/off Switch wired to its get/set Tauri command. Pigeon defaults OFF,
// Oracle defaults ON. This test defines the contract LabsView must satisfy.

import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";

const invokeMock = vi.fn(async (command: string, args?: Record<string, unknown>) => {
  switch (command) {
    case "get_pigeon_enabled":
      return false;
    case "get_oracle_enabled":
      return true;
    case "set_pigeon_enabled":
    case "set_oracle_enabled":
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
  it("renders a Pigeon and an Oracle card", async () => {
    await mount();
    expect(container.textContent).toContain("Pigeon");
    expect(container.textContent).toContain("Oracle");
  });

  it("loads each feature's enabled state on mount", async () => {
    await mount();
    expect(invokeMock).toHaveBeenCalledWith("get_pigeon_enabled");
    expect(invokeMock).toHaveBeenCalledWith("get_oracle_enabled");
    // Pigeon defaults off, Oracle defaults on — reflected in the switches.
    expect(switchByLabel("Toggle Pigeon").getAttribute("aria-checked")).toBe("false");
    expect(switchByLabel("Toggle Oracle").getAttribute("aria-checked")).toBe("true");
  });

  it("persists a toggle via set_pigeon_enabled when the Pigeon switch is clicked", async () => {
    await mount();
    const pigeon = switchByLabel("Toggle Pigeon");
    await act(async () => {
      pigeon.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(invokeMock).toHaveBeenCalledWith("set_pigeon_enabled", { enabled: true });
  });

  it("persists a toggle via set_oracle_enabled when the Oracle switch is clicked", async () => {
    await mount();
    const oracle = switchByLabel("Toggle Oracle");
    await act(async () => {
      oracle.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(invokeMock).toHaveBeenCalledWith("set_oracle_enabled", { enabled: false });
  });
});
