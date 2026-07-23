// @vitest-environment jsdom
//
// JUMP_TARGETS must no longer reference the cloud "Providers" area (S1). The
// view stays reachable by deep link, so this only asserts the search targets
// don't point at "providers" (in full or via a "providers#tab" deep link).
// Also: a risk flag whose resolved provider is in ALPHA_HIDDEN_PROVIDERS (e.g.
// Scaleway, Cloudflare) must NOT be counted in the badge nor shown in the list
// (TASK #8/#11) — it would otherwise deep-link to a tab that no longer exists.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { RiskFlag } from "../types/backend";
import { JUMP_TARGETS, viewTitles, Header } from "./Header";

// Mutable AppContext bag + a spy for requestView, hoisted so the vi.mock
// factory (also hoisted) can close over them.
const h = vi.hoisted(() => {
  const requestView = vi.fn();
  return {
    requestView,
    ctx: {
      activeView: "projects",
      cloudSnapshot: { risks: [] as RiskFlag[] },
      roleStatus: { role: "admin", isAdmin: true, provisioned: true },
    },
  };
});

vi.mock("../context/AppContext", () => ({
  useAppContext: () => h.ctx,
  useAppActions: () => ({ requestView: h.requestView, lock: vi.fn(), refreshRole: vi.fn() }),
  invokeBackendCommand: vi.fn(),
}));
vi.mock("../store/agentAttentionStore", () => ({
  useAgentAttentionStore: (selector: (s: { sessions: unknown[] }) => unknown) =>
    selector({ sessions: [] }),
}));
vi.mock("../store/dismissedRisks", () => ({
  useDismissedRisks: () => new Set<string>(),
  dismissRisk: vi.fn(),
  clearRisks: vi.fn(),
}));
vi.mock("../store/dismissedAttention", () => ({
  useDismissedAttention: () => new Set<string>(),
  dismissAttention: vi.fn(),
  clearAttentions: vi.fn(),
  attentionDismissKey: (s: { agentId: string }) => s.agentId,
}));
vi.mock("../hooks/useNow", () => ({ useNow: () => Date.now() }));

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let container: HTMLElement;
let root: Root | null = null;

beforeEach(() => {
  h.requestView.mockClear();
  h.ctx.cloudSnapshot = { risks: [] };
  container = document.createElement("div");
  document.body.appendChild(container);
});

afterEach(() => {
  act(() => root?.unmount());
  root = null;
  container.remove();
});

function renderHeader() {
  root = createRoot(container);
  act(() => {
    root!.render(createElement(Header));
  });
}

function openNotifications() {
  const bell = container.querySelector(
    '[aria-label="Notifications"]',
  ) as HTMLButtonElement | null;
  expect(bell).toBeTruthy();
  act(() => {
    bell!.click();
  });
}

describe("Header JUMP_TARGETS (S1)", () => {
  it("no jump target references the hidden providers view", () => {
    expect(JUMP_TARGETS.length).toBeGreaterThan(0);
    for (const entry of JUMP_TARGETS) {
      expect(entry.target).not.toBe("providers");
      expect(entry.target.startsWith("providers#")).toBe(false);
    }
  });
});

describe("Header viewTitles (S6)", () => {
  it("resolves the Skills view title so the header shows 'Skills'", () => {
    expect(viewTitles.skills).toBe("Skills");
  });

  it("resolves the Labs view title so the header shows 'Labs'", () => {
    expect(viewTitles.labs).toBe("Labs");
  });
});

describe("Header risk flags skip ALPHA_HIDDEN_PROVIDERS (B)", () => {
  it("neither counts nor shows a Scaleway-targeted risk while scaleway is hidden", () => {
    h.ctx.cloudSnapshot = {
      risks: [
        {
          id: "r-scale",
          severity: "medium",
          title: "Scaleway GPU capacity low",
          description: "scaleway region saturated",
          source: "scaleway",
          timestamp: "now",
        },
        {
          id: "r-secret",
          severity: "high",
          title: "Rotate API key",
          description: "secret rotation due",
          source: "secret",
          timestamp: "now",
        },
      ],
    };
    renderHeader();

    // Scaleway is hidden but the secrets risk is visible: badge must read "1",
    // never "2" (the hidden risk must not be counted).
    const badge = container.querySelector(
      '[aria-label="Notifications"] span',
    ) as HTMLElement | null;
    expect(badge?.textContent).toBe("1");

    // The dropdown shows only the visible (secrets) risk, not the hidden one.
    openNotifications();
    expect(container.textContent).toContain("Rotate API key");
    expect(container.textContent).not.toContain("Scaleway GPU capacity low");
  });
});
