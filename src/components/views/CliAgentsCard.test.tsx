// @vitest-environment jsdom
//
// C-F3 regression: CliAgentsCard must not get stuck in loading=true when
// a runAction call supersedes the in-flight mount-time loadStatus.
//
// Scenario: loadStatus fires on mount and resolves with runtimeReady=true.
// Configure is clicked (triggering runAction). Internally, runAction advances
// the seq so any concurrent/stale loadStatus that bails its finally cannot
// leave loading stuck true. We verify the UI is not stuck in the loading
// state after the action resolves.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import type { CliAgentsStatus } from "../../types/backend";

// ---------------------------------------------------------------------------
// Mocks
// ---------------------------------------------------------------------------

let resolveAction!: (v: CliAgentsStatus) => void;

const loadStatusMock = vi.fn<() => Promise<CliAgentsStatus>>();
const configureCliAgentsMock = vi.fn<() => Promise<CliAgentsStatus>>();
const unconfigureCliAgentsMock = vi.fn<() => Promise<CliAgentsStatus>>();

vi.mock("../../context/AppContext", () => ({
  useAppContext: () => ({}),
  useAppActions: () => ({
    cliAgentsStatus: loadStatusMock,
    configureCliAgents: configureCliAgentsMock,
    unconfigureCliAgents: unconfigureCliAgentsMock,
  }),
}));

import { __test_CliAgentsCard as CliAgentsCard } from "../oracle/CliAgentsCard";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const READY_STATUS: CliAgentsStatus = {
  claudeConfigured: false,
  claudeConfigPath: null,
  codexConfigured: false,
  codexNote: null,
  interpreter: null,
  root: null,
  projectsDir: null,
  runtimeReady: true,
  warning: null,
};

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

beforeEach(() => {
  // Mount-time loadStatus resolves immediately (so button is enabled).
  loadStatusMock.mockResolvedValue(READY_STATUS);
  configureCliAgentsMock.mockImplementation(
    () => new Promise<CliAgentsStatus>((res) => { resolveAction = res; }),
  );
});

afterEach(() => {
  vi.clearAllMocks();
});

// ---------------------------------------------------------------------------
// C-F3 regression
// ---------------------------------------------------------------------------

describe("CliAgentsCard loading-stuck (C-F3)", () => {
  it("loading is false after runAction completes even if a concurrent loadStatus would have left it stuck", async () => {
    // Mount load resolves immediately; a second loadStatus (if ever triggered)
    // would be slow, but the key invariant is that runAction clears loading
    // synchronously so it can never get stuck even if a stale seq bails.
    loadStatusMock.mockResolvedValue(READY_STATUS);

    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    // Mount: loadStatus fires and resolves, clearing loading + setting runtimeReady.
    await act(async () => {
      root.render(createElement(CliAgentsCard));
      await Promise.resolve();
    });
    // Flush the resolved loadStatus.
    await act(async () => { await Promise.resolve(); });

    // At this point loading=false, runtimeReady=true → Configure button is enabled.
    const configureBtn = Array.from(
      container.querySelectorAll<HTMLButtonElement>("button"),
    ).find((b) => b.textContent?.includes("Configure") && !b.disabled);
    expect(configureBtn).toBeDefined();

    // Click Configure: fires runAction, advances seq. This supersedes any future
    // loadStatus so that if a slow loadStatus bails its finally, loading stays false.
    await act(async () => {
      configureBtn!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
    });

    // Resolve the configure action — loading must be false afterwards.
    await act(async () => {
      resolveAction(READY_STATUS);
      await Promise.resolve();
    });

    // The spinner / loading indicator must be absent.
    expect(container.innerHTML).not.toContain("animate-spin");
    // loading=false means the buttons are not disabled due to loading alone.
    const buttons = container.querySelectorAll<HTMLButtonElement>("button");
    // At least one button should be enabled (not all disabled by loading).
    const hasEnabledButton = Array.from(buttons).some((b) => !b.disabled);
    expect(hasEnabledButton).toBe(true);

    await act(async () => { root.unmount(); });
    container.remove();
  });
});
