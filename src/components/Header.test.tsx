// @vitest-environment jsdom
//
// JUMP_TARGETS must not reference the removed cloud "Providers" area.
// viewTitles must still resolve Skills / Labs.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { JUMP_TARGETS, viewTitles, Header } from "./Header";

// Mutable AppContext bag + a spy for requestView, hoisted so the vi.mock
// factory (also hoisted) can close over them.
const h = vi.hoisted(() => {
  const requestView = vi.fn();
  return {
    requestView,
    ctx: {
      activeView: "projects",
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

describe("Header JUMP_TARGETS", () => {
  it("no jump target references the removed providers view", () => {
    expect(JUMP_TARGETS.length).toBeGreaterThan(0);
    for (const entry of JUMP_TARGETS) {
      expect(entry.target).not.toBe("providers");
      expect(entry.target.startsWith("providers#")).toBe(false);
      expect(entry.target).not.toBe("cloudflare");
      expect(entry.target).not.toBe("compute");
      expect(entry.target).not.toBe("budget");
    }
  });
});

describe("Header viewTitles", () => {
  it("resolves the Skills view title so the header shows 'Skills'", () => {
    expect(viewTitles.skills).toBe("Skills");
  });

  it("resolves the Labs view title so the header shows 'Labs'", () => {
    expect(viewTitles.labs).toBe("Labs");
  });
});

describe("Header notifications (agent attention only)", () => {
  it("renders without provider risk flags", () => {
    renderHeader();
    const bell = container.querySelector('[aria-label="Notifications"]');
    expect(bell).toBeTruthy();
    // No badge when no agents need attention.
    const badge = container.querySelector('[aria-label="Notifications"] span');
    expect(badge).toBeNull();
  });
});
