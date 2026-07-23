// @vitest-environment jsdom
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { act, createElement } from "react";
import type { PlanApprovalRequest } from "../../types/backend";

// Mock invoke so no Tauri runtime is needed.
const mockInvoke = vi.fn(async (..._args: unknown[]): Promise<unknown> => undefined);
vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (...args: unknown[]) => mockInvoke(...args),
  isTauriRuntime: () => false,
}));

import { PlanApprovalCard } from "./PlanApprovalCard";

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

describe("PlanApprovalCard", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    // Default: list returns a pending request; markdown fetch returns plan text.
    mockInvoke.mockImplementation(async (...args: unknown[]): Promise<unknown> => {
      const cmd = args[0] as string;
      if (cmd === "plan_approval_requests_list") return [req()];
      if (cmd === "get_plan_markdown") return "# Plan\n\nDo the thing.";
      return undefined;
    });
  });

  it("renders null (nothing) when there are no pending requests for the project", () => {
    // Provide pending requests for a DIFFERENT project — must stay invisible.
    const html = renderToStaticMarkup(
      <PlanApprovalCard projectId="other-project" requests={[req()]} />,
    );
    expect(html).toBe("");
  });

  it("renders the pending request title and agentId for the current project", () => {
    const html = renderToStaticMarkup(
      <PlanApprovalCard projectId="p1" requests={[req()]} />,
    );
    expect(html).toContain("Phase 2 — deploy backend");
    expect(html).toContain("coder-1");
  });

  it("renders Approve and Reject buttons for a pending request", () => {
    const html = renderToStaticMarkup(
      <PlanApprovalCard projectId="p1" requests={[req()]} />,
    );
    expect(html).toContain("Approve");
    expect(html).toContain("Reject");
  });

  it("renders nothing when the requests list is empty", () => {
    const html = renderToStaticMarkup(
      <PlanApprovalCard projectId="p1" requests={[]} />,
    );
    expect(html).toBe("");
  });

  it("renders nothing when requests is undefined", () => {
    const html = renderToStaticMarkup(
      <PlanApprovalCard projectId="p1" requests={undefined} />,
    );
    expect(html).toBe("");
  });

  it("does not show approved/rejected requests", () => {
    const html = renderToStaticMarkup(
      <PlanApprovalCard
        projectId="p1"
        requests={[
          req({ id: "r1", status: "approved" }),
          req({ id: "r2", status: "rejected" }),
        ]}
      />,
    );
    expect(html).toBe("");
  });
});

describe("PlanApprovalCard note textarea", () => {
  it("renders a note textarea for each pending request", () => {
    const html = renderToStaticMarkup(
      <PlanApprovalCard
        projectId="p1"
        requests={[req()]}
      />,
    );
    // The note area (textarea or input for the optional note).
    expect(html).toContain("textarea");
  });
});

describe("PlanApprovalCard onPendingCountChange", () => {
  it("fires with the correct count on mount with pending requests", async () => {
    const cb = vi.fn();
    const { createRoot } = await import("react-dom/client");
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(
        createElement(PlanApprovalCard, {
          projectId: "p1",
          requests: [req(), req({ id: "req-2" })],
          onPendingCountChange: cb,
        }),
      );
    });
    expect(cb).toHaveBeenCalledWith(2);
    root.unmount();
    container.remove();
  });

  it("fires with 0 when there are no matching requests", async () => {
    const cb = vi.fn();
    const { createRoot } = await import("react-dom/client");
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(
        createElement(PlanApprovalCard, {
          projectId: "other",
          requests: [req()],
          onPendingCountChange: cb,
        }),
      );
    });
    expect(cb).toHaveBeenCalledWith(0);
    root.unmount();
    container.remove();
  });

  it("fires with 0 when requests list is empty", async () => {
    const cb = vi.fn();
    const { createRoot } = await import("react-dom/client");
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(
        createElement(PlanApprovalCard, {
          projectId: "p1",
          requests: [],
          onPendingCountChange: cb,
        }),
      );
    });
    expect(cb).toHaveBeenCalledWith(0);
    root.unmount();
    container.remove();
  });
});

describe("PlanApprovalCard hidden prop", () => {
  it("renders nothing when hidden=true even with pending requests", () => {
    const html = renderToStaticMarkup(
      <PlanApprovalCard
        projectId="p1"
        requests={[req()]}
        hidden
      />,
    );
    expect(html).toBe("");
  });

  it("still fires onPendingCountChange when hidden", async () => {
    const cb = vi.fn();
    const { createRoot } = await import("react-dom/client");
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(
        createElement(PlanApprovalCard, {
          projectId: "p1",
          requests: [req()],
          hidden: true,
          onPendingCountChange: cb,
        }),
      );
    });
    expect(cb).toHaveBeenCalledWith(1);
    // Should render nothing (hidden).
    expect(container.innerHTML).toBe("");
    root.unmount();
    container.remove();
  });

  it("renders normally when hidden=false (default)", () => {
    const html = renderToStaticMarkup(
      <PlanApprovalCard
        projectId="p1"
        requests={[req()]}
        hidden={false}
      />,
    );
    expect(html).toContain("Approve");
  });
});

// ---- WARNING #3: no concurrent-load race after a resolve --------------------
//
// The bug: `resolve()` manually set `inFlightRef.current = false` right before
// `await load()`. That reset opened a window where the 5s poll could fire and run
// a SECOND `plan_approval_requests_list` concurrently with the resolve-triggered
// load. The fix: `load()` owns inFlightRef via try/finally; remove the manual reset.
// Regression: while the resolve-triggered list is still pending, fire the poll tick
// and assert NO second concurrent list call is made.

describe("PlanApprovalCard resolve does not race the poll", () => {
  let prevActEnv: unknown;

  beforeEach(() => {
    prevActEnv = (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT;
    (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
    // Fake the global timers (in jsdom env, window.setInterval === globalThis.setInterval)
    // so vi.advanceTimersByTime drives the component's poll interval.
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
    (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = prevActEnv;
  });

  it("a resolve forces a fresh list even while a poll is in flight (stale poll superseded)", async () => {
    // A poll's load() is in flight when resolve finishes. Resolve deliberately forces a
    // fresh refresh (`inFlightRef.current = false; await load()`) so the post-approve state
    // shows immediately rather than waiting a full poll interval. The forced load runs
    // concurrently with the held stale poll, and the monotonic generation token drops the
    // stale poll's late result so the fresh load wins (PushApprovalCard parity).
    let listInFlight = 0;
    let maxConcurrentList = 0;
    let listCallCount = 0;
    // Gate holding the FIRST list (mount + the held poll) open.
    const gates: Array<() => void> = [];

    mockInvoke.mockImplementation(async (...args: unknown[]): Promise<unknown> => {
      const cmd = args[0] as string;
      if (cmd === "plan_approval_requests_list") {
        listCallCount += 1;
        listInFlight += 1;
        maxConcurrentList = Math.max(maxConcurrentList, listInFlight);
        const myCall = listCallCount;
        // Mount load (call #1) resolves immediately. The poll load (call #2) is held
        // open so it is STILL IN FLIGHT when resolve runs.
        if (myCall >= 2) {
          await new Promise<void>((resolve) => {
            gates.push(resolve);
          });
        }
        listInFlight -= 1;
        return [req()];
      }
      if (cmd === "approve_plan_request") {
        return req({ status: "approved" });
      }
      if (cmd === "get_plan_markdown") return "# Plan";
      return undefined;
    });

    const { createRoot } = await import("react-dom/client");
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    // Uncontrolled mode => self-poll active. Mount load = list call #1.
    await act(async () => {
      root.render(createElement(PlanApprovalCard, { projectId: "p1" }));
    });
    expect(listCallCount).toBe(1);

    // Fire the poll -> list call #2, held open (inFlightRef stays true).
    await act(async () => {
      vi.advanceTimersByTime(POLL_INTERVAL_MS_TEST);
      await Promise.resolve();
    });
    expect(listCallCount).toBe(2);
    expect(listInFlight).toBe(1); // the poll load is in flight

    // Now click Approve. resolve() completes approve_plan_request then triggers a
    // load(). With the bug it resets inFlightRef and runs a 3rd CONCURRENT list.
    const approveBtn = Array.from(container.querySelectorAll("button")).find(
      (b) => b.textContent?.includes("Approve"),
    );
    expect(approveBtn).toBeTruthy();
    await act(async () => {
      approveBtn!.dispatchEvent(
        new window.MouseEvent("click", { bubbles: true, cancelable: true }),
      );
      await Promise.resolve();
      await Promise.resolve();
    });

    // Fixed (PushApprovalCard parity): resolve FORCES a fresh refresh even while a poll
    // is in flight (list call #3), so the just-approved state shows immediately instead of
    // waiting a full poll interval. The forced refresh runs concurrently with the held
    // stale poll (maxConcurrent === 2); the generation token then drops the stale poll's
    // result when it finally returns, so the fresh load wins.
    expect(listCallCount).toBe(3);
    expect(maxConcurrentList).toBe(2);

    // Release held list(s) and unmount cleanly.
    gates.forEach((g) => g());
    await act(async () => {
      await Promise.resolve();
      root.unmount();
    });
    container.remove();
  });
});

// Mirrors POLL_INTERVAL_MS in the component (kept local to avoid importing it).
const POLL_INTERVAL_MS_TEST = 5000;
