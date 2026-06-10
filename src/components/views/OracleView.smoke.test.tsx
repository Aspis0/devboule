// @vitest-environment jsdom
//
// Smoke test for the restored standalone OracleView: it must render the ask
// section (search input + Ask button + seed-question chips) AND mount the
// OracleAdminPanel below it. The admin panel is mocked to a lightweight marker
// so this only proves the page composition, not the admin internals.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";

const askOracle = vi.fn(async (_query: string, _limit?: number) => ({
  answer: "ok",
  citations: [],
}));
const requestView = vi.fn();

// Provider configured → ask controls enabled. The view reads askOracle,
// requestView, oracleLlmSettings, secretStatuses from context.
vi.mock("../../context/AppContext", () => ({
  useAppContext: () => ({
    askOracle,
    requestView,
    oracleLlmSettings: { apiKeyConfigured: true },
    secretStatuses: [],
  }),
}));

vi.mock("../oracle/OracleAdminPanel", () => ({
  OracleAdminPanel: () =>
    createElement("div", { "data-testid": "oracle-admin-panel" }, "admin"),
}));

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let container: HTMLElement;

beforeEach(() => {
  askOracle.mockClear();
  requestView.mockClear();
  container = document.createElement("div");
  document.body.appendChild(container);
});

describe("OracleView (restored standalone page)", () => {
  it("renders the ask section and the admin panel", async () => {
    const { OracleView } = await import("./OracleView");
    await act(async () => {
      createRoot(container).render(createElement(OracleView));
    });

    // Page header.
    expect(container.textContent).toContain("Oracle");

    // Ask input.
    const input = container.querySelector<HTMLInputElement>(
      'input[placeholder="Ask about your codebase…"]',
    );
    expect(input).toBeTruthy();

    // Ask button (enabled because the provider is configured + seed query >= 3).
    const askBtn = Array.from(container.querySelectorAll("button")).find(
      (b) => b.textContent?.trim() === "Ask",
    );
    expect(askBtn).toBeTruthy();
    expect((askBtn as HTMLButtonElement).disabled).toBe(false);

    // At least one seed-question chip.
    const chips = Array.from(container.querySelectorAll("button")).filter((b) =>
      b.textContent?.includes("?"),
    );
    expect(chips.length).toBeGreaterThan(0);

    // The admin panel is mounted below the ask section.
    expect(
      container.querySelector('[data-testid="oracle-admin-panel"]'),
    ).not.toBeNull();
  });

  it("runs the query via askOracle when Ask is clicked", async () => {
    const { OracleView } = await import("./OracleView");
    await act(async () => {
      createRoot(container).render(createElement(OracleView));
    });

    const askBtn = Array.from(container.querySelectorAll("button")).find(
      (b) => b.textContent?.trim() === "Ask",
    )!;

    await act(async () => {
      askBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    expect(askOracle).toHaveBeenCalledTimes(1);
    // limit 8 per the shared ask-flow contract.
    expect(askOracle.mock.calls[0][1]).toBe(8);
  });
});
