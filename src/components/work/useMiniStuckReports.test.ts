// @vitest-environment jsdom

import { describe, it, expect, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { useMiniStuckReports, type MiniStuckDeps } from "./useMiniStuckReports";
import type { MiniStuckReport } from "./miniStuckModel";

(
  globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

function makeReport(
  overrides: Partial<MiniStuckReport> = {},
): MiniStuckReport {
  return {
    taskId: "T1",
    agentId: "agent-1",
    reason: "timeout",
    attempts: 1,
    lastOutput: "",
    filesTouched: [],
    ...overrides,
  };
}

describe("useMiniStuckReports", () => {
  it("adds an incoming event to reports", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    let emit: ((e: { payload: MiniStuckReport }) => void) | null = null;
    let latest = { reports: [] as MiniStuckReport[], dismiss: (_: string) => {} };

    const deps: MiniStuckDeps = {
      listen: vi.fn(async (_channel, handler) => {
        emit = handler;
        return () => {};
      }),
    };

    function Harness() {
      latest = useMiniStuckReports(deps);
      return null;
    }

    await act(async () => {
      root.render(createElement(Harness));
    });
    expect(latest.reports).toHaveLength(0);

    await act(async () => {
      emit?.({ payload: makeReport({ taskId: "T1" }) });
    });
    expect(latest.reports).toHaveLength(1);
    expect(latest.reports[0].taskId).toBe("T1");

    await act(async () => {
      root.unmount();
    });
    container.remove();
  });

  it("caps reports at 5, dropping the oldest", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    let emit: ((e: { payload: MiniStuckReport }) => void) | null = null;
    let latest = { reports: [] as MiniStuckReport[], dismiss: (_: string) => {} };

    const deps: MiniStuckDeps = {
      listen: vi.fn(async (_channel, handler) => {
        emit = handler;
        return () => {};
      }),
    };

    function Harness() {
      latest = useMiniStuckReports(deps);
      return null;
    }

    await act(async () => {
      root.render(createElement(Harness));
    });

    // Emit 7 reports; only the newest 5 should survive.
    for (let i = 1; i <= 7; i++) {
      await act(async () => {
        emit?.({ payload: makeReport({ taskId: `T${i}` }) });
      });
    }

    expect(latest.reports).toHaveLength(5);
    // Newest first: T7, T6, T5, T4, T3
    expect(latest.reports.map((r) => r.taskId)).toEqual([
      "T7",
      "T6",
      "T5",
      "T4",
      "T3",
    ]);

    await act(async () => {
      root.unmount();
    });
    container.remove();
  });

  it("dismiss removes exactly that report", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    let emit: ((e: { payload: MiniStuckReport }) => void) | null = null;
    let latest = { reports: [] as MiniStuckReport[], dismiss: (_: string) => {} };

    const deps: MiniStuckDeps = {
      listen: vi.fn(async (_channel, handler) => {
        emit = handler;
        return () => {};
      }),
    };

    function Harness() {
      latest = useMiniStuckReports(deps);
      return null;
    }

    await act(async () => {
      root.render(createElement(Harness));
    });

    await act(async () => {
      emit?.({ payload: makeReport({ taskId: "A" }) });
    });
    await act(async () => {
      emit?.({ payload: makeReport({ taskId: "B" }) });
    });
    expect(latest.reports).toHaveLength(2);

    await act(async () => {
      latest.dismiss("A");
    });

    expect(latest.reports).toHaveLength(1);
    expect(latest.reports[0].taskId).toBe("B");

    await act(async () => {
      root.unmount();
    });
    container.remove();
  });

  it("unmount calls the injected unlisten function", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    const unlistenFn = vi.fn();

    const deps: MiniStuckDeps = {
      listen: vi.fn(async () => unlistenFn),
    };

    function Harness() {
      useMiniStuckReports(deps);
      return null;
    }

    await act(async () => {
      root.render(createElement(Harness));
    });

    await act(async () => {
      root.unmount();
    });

    expect(unlistenFn).toHaveBeenCalledTimes(1);
    container.remove();
  });

  it("report with projectId is received and dismissable", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    let emit: ((e: { payload: MiniStuckReport }) => void) | null = null;
    let latest = { reports: [] as MiniStuckReport[], dismiss: (_: string) => {} };

    const deps: MiniStuckDeps = {
      listen: vi.fn(async (_channel, handler) => {
        emit = handler;
        return () => {};
      }),
    };

    function Harness() {
      latest = useMiniStuckReports(deps);
      return null;
    }

    await act(async () => {
      root.render(createElement(Harness));
    });

    // Emit a report carrying a projectId — it must appear like any other report.
    await act(async () => {
      emit?.({
        payload: makeReport({ taskId: "app-T1", agentId: "app-user", projectId: "p42" }),
      });
    });
    expect(latest.reports).toHaveLength(1);
    expect(latest.reports[0].agentId).toBe("app-user");
    expect(latest.reports[0].taskId).toBe("app-T1");
    expect(latest.reports[0].projectId).toBe("p42");

    // Dismiss it — it must be removed.
    await act(async () => {
      latest.dismiss("app-T1");
    });
    expect(latest.reports).toHaveLength(0);

    await act(async () => {
      root.unmount();
    });
    container.remove();
  });
});
