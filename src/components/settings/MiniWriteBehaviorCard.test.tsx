// @vitest-environment jsdom
//
// E1/E2/E3 — MiniWriteBehaviorCard. The card loads its policy + coverage on mount
// (async effects) and persists on change, so this uses jsdom + createRoot + act
// (the repo's interactive-test pattern, mirroring ProvidersModelsTab.test.tsx).
// We mock invokeBackendCommand to drive get/set_mini_write_behavior +
// get_agentic_coverage_languages and to capture the set call's payload.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";

import type { MiniWriteBehavior } from "../../types/config";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let persistedPolicy: MiniWriteBehavior = "auto";
let coverage: string[] = ["Python", "TypeScript/JavaScript"];
let coverageThrows = false;
const setCalls: MiniWriteBehavior[] = [];
// When set, get_mini_write_behavior awaits this gate before resolving so a test can
// observe the pre-load (!loaded) window deterministically.
let pendingPolicyLoad: Promise<void> | null = null;
let resolvePolicyLoad: (() => void) | null = null;

const invokeMock = vi.fn(async (name: string, args?: Record<string, unknown>) => {
  if (name === "get_mini_write_behavior") {
    if (pendingPolicyLoad) await pendingPolicyLoad;
    return persistedPolicy;
  }
  if (name === "set_mini_write_behavior") {
    const next = args?.behavior as MiniWriteBehavior;
    setCalls.push(next);
    persistedPolicy = next;
    return next;
  }
  if (name === "get_agentic_coverage_languages") {
    if (coverageThrows) throw new Error("coverage unavailable");
    return coverage;
  }
  return null;
});

vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (name: string, args?: Record<string, unknown>) =>
    invokeMock(name, args),
}));

import { MiniWriteBehaviorCard } from "./MiniWriteBehaviorCard";

let container: HTMLDivElement;
let root: Root;

async function mount() {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () => {
    root.render(createElement(MiniWriteBehaviorCard));
  });
  // Flush the two mount effects (policy load + coverage load).
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

beforeEach(() => {
  persistedPolicy = "auto";
  coverage = ["Python", "TypeScript/JavaScript"];
  coverageThrows = false;
  setCalls.length = 0;
  pendingPolicyLoad = null;
  resolvePolicyLoad = null;
  invokeMock.mockClear();
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

describe("MiniWriteBehaviorCard", () => {
  it("renders the three policy options as a radiogroup", async () => {
    await mount();
    const radios = container.querySelectorAll('[role="radio"]');
    expect(radios.length).toBe(3);
    const html = container.innerHTML;
    expect(html).toContain("Safe");
    expect(html).toContain("Auto");
    expect(html).toContain("Agentic allowed");
    expect(container.querySelector('[role="radiogroup"]')).not.toBeNull();
  });

  it("reflects the persisted policy as the checked radio", async () => {
    persistedPolicy = "agenticAllowed";
    await mount();
    const checked = container.querySelector('[role="radio"][aria-checked="true"]');
    expect(checked).not.toBeNull();
    expect(checked?.textContent).toContain("Agentic allowed");
    // Only one radio is checked.
    expect(
      container.querySelectorAll('[role="radio"][aria-checked="true"]').length,
    ).toBe(1);
  });

  it("defaults to Auto checked when no policy is persisted", async () => {
    persistedPolicy = "auto";
    await mount();
    const checked = container.querySelector('[role="radio"][aria-checked="true"]');
    expect(checked?.textContent).toContain("Auto");
  });

  it("calls set_mini_write_behavior with the chosen policy on change", async () => {
    await mount();
    const radios = Array.from(
      container.querySelectorAll('[role="radio"]'),
    ) as HTMLButtonElement[];
    // Index 0 = Safe (the most-restrictive option).
    const safe = radios[0];
    expect(safe.textContent).toContain("Safe");
    await act(async () => {
      safe.click();
      await Promise.resolve();
    });
    expect(setCalls).toEqual(["safe"]);
    // The selection now reflects the new policy.
    const checked = container.querySelector('[role="radio"][aria-checked="true"]');
    expect(checked?.textContent).toContain("Safe");
  });

  it("reverts the selection when the save fails", async () => {
    await mount();
    // Make the next set call reject.
    invokeMock.mockImplementationOnce(async () => {
      throw new Error("write failed");
    });
    const radios = Array.from(
      container.querySelectorAll('[role="radio"]'),
    ) as HTMLButtonElement[];
    const agentic = radios[2]; // Agentic allowed
    await act(async () => {
      agentic.click();
      await Promise.resolve();
    });
    // The optimistic selection reverted to the previous (Auto) and the error shows.
    const checked = container.querySelector('[role="radio"][aria-checked="true"]');
    expect(checked?.textContent).toContain("Auto");
    expect(container.innerHTML).toContain("write failed");
  });

  it("does NOT persist a click while the policy is still loading (!loaded)", async () => {
    // The persisted policy is a non-Auto value that has not resolved yet. The control
    // must be inert (disabled) so a premature click can't overwrite "safe" with the
    // still-default "auto" — the original pre-load write race.
    persistedPolicy = "safe";
    pendingPolicyLoad = new Promise<void>((resolve) => {
      resolvePolicyLoad = resolve;
    });
    await mount();

    const radios = Array.from(
      container.querySelectorAll('[role="radio"]'),
    ) as HTMLButtonElement[];
    // Every radio is disabled until the persisted value has loaded.
    expect(radios.every((r) => r.disabled)).toBe(true);
    const auto = radios[1]; // Auto — the still-default selection.
    expect(auto.textContent).toContain("Auto");
    await act(async () => {
      auto.click();
      await Promise.resolve();
    });
    // No write was issued — the persisted "safe" is untouched.
    expect(setCalls).toEqual([]);

    // Once the load resolves the control becomes interactive again.
    await act(async () => {
      resolvePolicyLoad?.();
      await Promise.resolve();
      await Promise.resolve();
    });
    const enabled = Array.from(
      container.querySelectorAll('[role="radio"]'),
    ) as HTMLButtonElement[];
    expect(enabled.every((r) => r.disabled)).toBe(false);
    const checked = container.querySelector('[role="radio"][aria-checked="true"]');
    expect(checked?.textContent).toContain("Safe");
  });

  it("shows the coverage list from get_agentic_coverage_languages", async () => {
    coverage = ["Go", "Python"];
    await mount();
    const html = container.innerHTML;
    expect(html).toContain("Agentic-iterative coverage:");
    expect(html).toContain("Go, Python");
    expect(html).toContain("depends on the detected project");
  });

  it("shows 'none' coverage when the set is empty and degrades when unavailable", async () => {
    coverage = [];
    await mount();
    expect(container.innerHTML).toContain("Agentic-iterative coverage:");
    expect(container.innerHTML).toContain("none");

    // Re-mount with the coverage command throwing -> the coverage block is omitted.
    act(() => root.unmount());
    container.remove();
    coverageThrows = true;
    await mount();
    expect(container.innerHTML).not.toContain("Agentic-iterative coverage:");
  });

  it("toggles the 'How this works' explainer", async () => {
    await mount();
    // Collapsed by default: the explainer body copy is not in the DOM. (Use a string
    // unique to the explainer — "Agentic-iterative coverage:" is always present.)
    expect(container.innerHTML).not.toContain("human-gated");
    const toggle = Array.from(container.querySelectorAll("button")).find((b) =>
      b.textContent?.includes("How this works"),
    ) as HTMLButtonElement;
    expect(toggle).toBeDefined();
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    await act(async () => {
      toggle.click();
      await Promise.resolve();
    });
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    const html = container.innerHTML;
    expect(html).toContain("Emit-edits");
    expect(html).toContain("deterministic gate");
    expect(html).toContain("human-gated");
    expect(html).toContain("sandboxed");
    expect(html).toContain("degrades gracefully");
  });
});
