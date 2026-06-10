// @vitest-environment jsdom
//
// The standalone Oracle view was RESTORED. The Header jump-search "Oracle" entry
// must reach the "oracle" view again (the primary, first-listed Oracle target),
// while an additional "Oracle (Ask)" entry may still point at Polis (the
// parchment ask panel). This test types "oracle" and asserts the FIRST match
// navigates to requestView("oracle", ...).

import { describe, it, expect, vi } from "vitest";

const requestView = vi.fn();

vi.mock("../context/AppContext", () => ({
  useAppContext: () => ({
    activeView: "projects",
    cloudSnapshot: null,
    roleStatus: { role: "admin", isAdmin: true, provisioned: true },
  }),
  useAppActions: () => ({
    requestView,
    lock: vi.fn(),
    refreshRole: vi.fn(),
  }),
  invokeBackendCommand: vi.fn(),
}));

vi.mock("../utils/deepLink", () => ({
  attentionBellTarget: vi.fn(() => ({ view: "projects", tab: null })),
  parseViewTarget: vi.fn((t: string) => {
    const [view, tab] = t.split("#");
    return { view, tab: tab ?? null };
  }),
}));

vi.mock("../store/agentAttentionStore", () => ({
  useAgentAttentionStore: vi.fn(() => []),
}));

vi.mock("./agents/agentFleet", () => ({
  attentionSessions: vi.fn(() => []),
}));

vi.mock("./agents/attentionNotifier", () => ({
  stripSpoofChars: (s: string) => s,
}));

vi.mock("./headerBadge", () => ({
  combineBadgeCount: vi.fn(() => 0),
}));

vi.mock("../hooks/useNow", () => ({
  useNow: vi.fn(() => Date.now()),
}));

import { act, createElement } from "react";
import { createRoot } from "react-dom/client";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

function setReactInputValue(input: HTMLInputElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(
    window.HTMLInputElement.prototype,
    "value",
  )?.set;
  setter?.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

describe("Header jump-search Oracle (restored view)", () => {
  it("the primary Oracle jump entry navigates to the oracle view", async () => {
    requestView.mockClear();
    const { Header } = await import("./Header");
    const container = document.createElement("div");
    document.body.appendChild(container);

    await act(async () => {
      createRoot(container).render(createElement(Header));
    });

    const input = container.querySelector<HTMLInputElement>(
      'input[placeholder="Jump to view..."]',
    )!;
    expect(input).toBeTruthy();

    await act(async () => {
      setReactInputValue(input, "oracle");
    });

    // Find the autocomplete button whose label is exactly "Oracle".
    const buttons = Array.from(container.querySelectorAll("button"));
    const oracleBtn = buttons.find((b) => b.textContent?.trim() === "Oracle");
    expect(oracleBtn).toBeTruthy();

    await act(async () => {
      oracleBtn!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    expect(requestView).toHaveBeenCalledWith("oracle", null);
  });

  it("still offers an additional 'Oracle (Ask)' entry that targets Polis", async () => {
    requestView.mockClear();
    const { Header } = await import("./Header");
    const container = document.createElement("div");
    document.body.appendChild(container);

    await act(async () => {
      createRoot(container).render(createElement(Header));
    });

    const input = container.querySelector<HTMLInputElement>(
      'input[placeholder="Jump to view..."]',
    )!;

    await act(async () => {
      setReactInputValue(input, "ask");
    });

    const buttons = Array.from(container.querySelectorAll("button"));
    const askBtn = buttons.find(
      (b) => b.textContent?.trim() === "Oracle (Ask)",
    );
    expect(askBtn).toBeTruthy();

    await act(async () => {
      askBtn!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    expect(requestView).toHaveBeenCalledWith("polis", null);
  });
});
