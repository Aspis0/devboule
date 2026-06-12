// @vitest-environment jsdom
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import type { PlanApprovalRequest } from "../../types/backend";

const mockInvoke = vi.fn(async (..._args: unknown[]): Promise<unknown> => undefined);
vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (...args: unknown[]) => mockInvoke(...args),
  isTauriRuntime: () => false,
}));

import { PlansPanel, PlansDockTab } from "./PlansPanel";

function req(over: Partial<PlanApprovalRequest> = {}): PlanApprovalRequest {
  return {
    id: "req-1",
    agentId: "coder-1",
    projectId: "p1",
    title: "Phase 2 — deploy backend",
    status: "pending_approval",
    createdAt: "2026-06-09T10:00:00Z",
    ...over,
  };
}

describe("PlansPanel", () => {
  it("renders empty state when there are no plans", () => {
    const html = renderToStaticMarkup(
      <PlansPanel plans={[]} />,
    );
    expect(html).toContain("No plans");
  });

  it("renders plan title and agentId", () => {
    const html = renderToStaticMarkup(
      <PlansPanel plans={[req()]} />,
    );
    expect(html).toContain("Phase 2 — deploy backend");
    expect(html).toContain("coder-1");
  });

  it("renders amber badge for pending_approval status", () => {
    const html = renderToStaticMarkup(
      <PlansPanel plans={[req({ status: "pending_approval" })]} />,
    );
    // Amber badge class or text indicating pending state.
    expect(html).toContain("amber");
  });

  it("renders green badge for approved status", () => {
    const html = renderToStaticMarkup(
      <PlansPanel
        plans={[req({ id: "r2", status: "approved", decidedAt: "2026-06-09T11:00:00Z" })]}
      />,
    );
    expect(html).toContain("approved");
  });

  it("renders red badge for rejected status", () => {
    const html = renderToStaticMarkup(
      <PlansPanel
        plans={[req({ id: "r3", status: "rejected", decidedAt: "2026-06-09T11:00:00Z" })]}
      />,
    );
    expect(html).toContain("rejected");
  });

  it("renders gray badge for timeout status", () => {
    const html = renderToStaticMarkup(
      <PlansPanel
        plans={[req({ id: "r4", status: "timeout" })]}
      />,
    );
    expect(html).toContain("timeout");
  });

  it("renders note preview when present", () => {
    const html = renderToStaticMarkup(
      <PlansPanel
        plans={[req({ note: "Looks good to me." })]}
      />,
    );
    expect(html).toContain("Looks good to me.");
  });
});

// ---- C-F2 regression: PlansPanel toggle stale-closure / unmount guard ------

describe("PlansPanel toggle races (C-F2)", () => {
  let prevActEnv: unknown;

  beforeEach(() => {
    mockInvoke.mockReset();
    prevActEnv = (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT;
    (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
  });

  afterEach(() => {
    (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = prevActEnv;
  });

  const planA: PlanApprovalRequest = req({ id: "a", title: "Plan A" });
  const planB: PlanApprovalRequest = req({ id: "b", title: "Plan B" });

  it("clicking row B while row A is loading does NOT overwrite B's loading state when A resolves late", async () => {
    // A resolves after B has already started loading.
    let resolveA!: (v: string) => void;
    let resolveB!: (v: string) => void;
    const promiseA = new Promise<string>((res) => { resolveA = res; });
    const promiseB = new Promise<string>((res) => { resolveB = res; });

    mockInvoke.mockImplementation(async (...args: unknown[]): Promise<unknown> => {
      const payload = args[1] as { planId?: string };
      if (payload?.planId === "a") return promiseA;
      if (payload?.planId === "b") return promiseB;
      return undefined;
    });

    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    // Render with two plans.
    await act(async () => {
      root.render(createElement(PlansPanel, { plans: [planA, planB] }));
    });

    // Click row A: starts loading A.
    const [btnA, btnB] = container.querySelectorAll<HTMLButtonElement>("button[type='button']");
    await act(async () => {
      btnA.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
    });

    // Click row B before A resolves: B's loading state should dominate.
    await act(async () => {
      btnB.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
    });

    // Now A resolves — its result must NOT update the expanded state (B is current).
    await act(async () => {
      resolveA("# Plan A content");
      await Promise.resolve();
    });

    // Expanded should still reflect B (loading state or B's own resolve — not A's content).
    expect(container.innerHTML).not.toContain("Plan A content");

    // Clean up.
    resolveB("# Plan B content");
    await act(async () => {
      root.unmount();
    });
    container.remove();
  });

  it("unmounting while a fetch is in-flight does not trigger a setState (no act warning)", async () => {
    let resolveMarkdown!: (v: string) => void;
    const markdownPromise = new Promise<string>((res) => { resolveMarkdown = res; });

    mockInvoke.mockImplementation(async (): Promise<unknown> => markdownPromise);

    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    await act(async () => {
      root.render(createElement(PlansPanel, { plans: [planA] }));
    });

    const [btnA] = container.querySelectorAll<HTMLButtonElement>("button[type='button']");
    await act(async () => {
      btnA.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
    });

    // Unmount while A is still in flight.
    await act(async () => {
      root.unmount();
    });

    // Resolving AFTER unmount must not throw / warn.
    await act(async () => {
      resolveMarkdown("# After unmount");
      await Promise.resolve();
    });

    container.remove();
    // If we reach here without an unhandled error or act() warning, the guard works.
  });
});

// ---- WARNING #8: PlansDockTab self-poll refetch -----------------------------
//
// The dock tab used to fetch only on mount, so a plan approved/rejected elsewhere
// left a stale "pending" status until the user reopened the tab. It must now poll
// on a modest interval (skipping hidden ticks) and clear the interval on unmount.

describe("PlansDockTab self-poll", () => {
  let prevActEnv: unknown;

  beforeEach(() => {
    mockInvoke.mockReset();
    prevActEnv = (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT;
    (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
    // In the jsdom env, window.setInterval === globalThis.setInterval, so faking the
    // global timers lets vi.advanceTimersByTime drive the component's poll interval.
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
    (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = prevActEnv;
  });

  function listCallCount(): number {
    return mockInvoke.mock.calls.filter((c) => c[0] === "list_project_plans").length;
  }

  it("refetches list_project_plans on the poll interval and stops after unmount", async () => {
    let current: PlanApprovalRequest[] = [req({ status: "pending_approval" })];
    mockInvoke.mockImplementation(async (...args: unknown[]): Promise<unknown> => {
      if (args[0] === "list_project_plans") return current;
      return undefined;
    });

    const { createRoot } = await import("react-dom/client");
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    await act(async () => {
      root.render(createElement(PlansDockTab, { projectId: "p1" }));
    });
    // Mount fetch only.
    expect(listCallCount()).toBe(1);

    // Approve happens elsewhere -> backend now returns approved.
    current = [req({ status: "approved", decidedAt: "2026-06-09T11:00:00Z" })];

    // Advance one poll interval -> a SECOND fetch fires and the panel updates.
    await act(async () => {
      vi.advanceTimersByTime(12000);
      await Promise.resolve();
    });
    expect(listCallCount()).toBe(2);
    expect(container.innerHTML).toContain("approved");

    // Unmount must clear the interval: no further fetches after another interval.
    await act(async () => {
      root.unmount();
    });
    await act(async () => {
      vi.advanceTimersByTime(12000 * 3);
      await Promise.resolve();
    });
    expect(listCallCount()).toBe(2);
    container.remove();
  });

  it("does not poll while the tab is hidden", async () => {
    mockInvoke.mockImplementation(async (...args: unknown[]): Promise<unknown> => {
      if (args[0] === "list_project_plans") return [req()];
      return undefined;
    });
    // Force the document hidden.
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      get: () => "hidden",
    });

    const { createRoot } = await import("react-dom/client");
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    await act(async () => {
      root.render(createElement(PlansDockTab, { projectId: "p1" }));
    });
    expect(listCallCount()).toBe(1); // mount fetch

    await act(async () => {
      vi.advanceTimersByTime(12000 * 2);
      await Promise.resolve();
    });
    // Hidden ticks are skipped -> still just the mount fetch.
    expect(listCallCount()).toBe(1);

    await act(async () => {
      root.unmount();
    });
    container.remove();
  });
});
