// @vitest-environment jsdom
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import type { ProjectDetail, ProjectTask, ProjectTaskStatus } from "../../types/backend";

const mockInvoke = vi.fn(async (..._args: unknown[]): Promise<unknown> => undefined);
vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (...args: unknown[]) => mockInvoke(...args),
  isTauriRuntime: () => false,
}));

import { PlanExecutionView } from "./PlanExecutionView";
import { buildPlanExecutionModel } from "./planExecutionModel";
import {
  PlanExecutionBody,
  canRetry,
  canSkip,
} from "./PlanExecutionView";

// ---- fixtures ---------------------------------------------------------------

function task(
  over: Partial<ProjectTask> & { id: string; status: ProjectTaskStatus },
): ProjectTask {
  return {
    title: `Task ${over.id}`,
    priority: null,
    assignee: null,
    due: null,
    linkedResources: [],
    updatedAt: "2026-06-15T10:00:00Z",
    suspectFileIds: [],
    ...over,
  };
}

function makeDetail(tasks: ProjectTask[]): ProjectDetail {
  return {
    metadata: {
      id: "p1",
      title: "Test Project",
      status: "active",
      createdAt: "2026-01-01T00:00:00Z",
      updatedAt: "2026-06-01T00:00:00Z",
      rootPath: null,
      linkedResources: [],
    } as unknown as ProjectDetail["metadata"],
    state: {
      version: 1,
      tasks,
      notes: [],
    },
    markdown: "",
    revision: "r1",
    path: "/p1",
    modifiedAt: null,
    liveStatus: { resources: [], checkedAt: "" },
    gitStatus: null as unknown as ProjectDetail["gitStatus"],
  };
}

const PLAN_ID = "plan-alpha";
const PLAN_TASKS: ProjectTask[] = [
  task({ id: "T1", status: "done", planId: PLAN_ID }),
  task({ id: "T2", status: "review", planId: PLAN_ID }),
  task({ id: "T3", status: "wip", planId: PLAN_ID, dependsOn: ["T1", "T2"] }),
  task({ id: "T4", status: "todo", planId: PLAN_ID, dependsOn: ["T3"] }),
  task({ id: "T5", status: "blocked", planId: PLAN_ID }),
];
const NON_PLAN_TASKS: ProjectTask[] = [
  task({ id: "N1", status: "todo" }),
  task({ id: "N2", status: "wip" }),
];
const MIXED_TASKS = [...PLAN_TASKS, ...NON_PLAN_TASKS];

// ---- PlanExecutionBody static render tests ----------------------------------

describe("PlanExecutionBody static render", () => {
  it("renders 'No active plan tasks' when model has null activePlanId", () => {
    const model = buildPlanExecutionModel([]);
    const html = renderToStaticMarkup(
      createElement(PlanExecutionBody, { model }),
    );
    expect(html).toContain("No active plan");
  });

  it("renders only plan-tagged tasks (not non-plan tasks)", () => {
    const model = buildPlanExecutionModel(MIXED_TASKS);
    const html = renderToStaticMarkup(
      createElement(PlanExecutionBody, { model }),
    );
    // Plan tasks IDs must appear
    expect(html).toContain("T1");
    expect(html).toContain("T2");
    expect(html).toContain("T3");
    expect(html).toContain("T4");
    expect(html).toContain("T5");
    // Non-plan task IDs must NOT appear
    expect(html).not.toContain("N1");
    expect(html).not.toContain("N2");
  });

  it("renders the correct status glyphs", () => {
    const model = buildPlanExecutionModel(PLAN_TASKS);
    const html = renderToStaticMarkup(
      createElement(PlanExecutionBody, { model }),
    );
    expect(html).toContain("✅"); // T1 done
    expect(html).toContain("🟡"); // T2 review
    expect(html).toContain("🔵"); // T3 wip
    expect(html).toContain("⏳"); // T4 todo
    expect(html).toContain("🔴"); // T5 blocked
  });

  it("renders dep labels for tasks that have dependsOn", () => {
    const model = buildPlanExecutionModel(PLAN_TASKS);
    const html = renderToStaticMarkup(
      createElement(PlanExecutionBody, { model }),
    );
    expect(html).toContain("dep: T1, T2"); // T3
    expect(html).toContain("dep: T3");    // T4
  });

  it("does NOT render a dep label for tasks with no dependsOn", () => {
    const model = buildPlanExecutionModel(PLAN_TASKS);
    const html = renderToStaticMarkup(
      createElement(PlanExecutionBody, { model }),
    );
    // T1/T2 have no deps — their IDs appear but they should not have a "dep:" label
    // We verify the total dep-label count (T3 and T4 only = 2 occurrences of "dep:").
    const occurrences = (html.match(/dep:/g) ?? []).length;
    expect(occurrences).toBe(2);
  });

  it("renders done/total footer", () => {
    const model = buildPlanExecutionModel(PLAN_TASKS);
    // done = T1 (done) + T2 (review) = 2; total = 5
    const html = renderToStaticMarkup(
      createElement(PlanExecutionBody, { model }),
    );
    expect(html).toContain("2/5 done");
  });

  it("footer shows all done when the plan is fully terminal", () => {
    const allDone = [
      task({ id: "D1", status: "done", planId: "fin" }),
      task({ id: "D2", status: "review", planId: "fin" }),
    ];
    const model = buildPlanExecutionModel(allDone);
    const html = renderToStaticMarkup(
      createElement(PlanExecutionBody, { model }),
    );
    expect(html).toContain("2/2 done");
  });

  it("active plan selection: renders the in-progress plan, not the all-done plan", () => {
    const planDone = "plan-done";
    const planActive = "plan-active";
    const tasks: ProjectTask[] = [
      task({ id: "F1", status: "done", planId: planDone, updatedAt: "2026-06-10T00:00:00Z" }),
      task({ id: "A1", status: "wip", planId: planActive, updatedAt: "2026-06-15T00:00:00Z" }),
      task({ id: "A2", status: "todo", planId: planActive, updatedAt: "2026-06-15T01:00:00Z" }),
    ];
    const model = buildPlanExecutionModel(tasks);
    const html = renderToStaticMarkup(
      createElement(PlanExecutionBody, { model }),
    );
    expect(html).toContain("A1");
    expect(html).toContain("A2");
    expect(html).not.toContain("F1");
  });

  it("renders task titles", () => {
    const tasks = [
      task({ id: "X1", status: "todo", planId: "p", title: "Implement the thing" }),
    ];
    const model = buildPlanExecutionModel(tasks);
    const html = renderToStaticMarkup(
      createElement(PlanExecutionBody, { model }),
    );
    expect(html).toContain("Implement the thing");
  });
});

// ---- PlanExecutionView live-poll tests --------------------------------------

describe("PlanExecutionView live poll", () => {
  let prevActEnv: unknown;

  beforeEach(() => {
    mockInvoke.mockReset();
    prevActEnv = (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT;
    (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
    (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = prevActEnv;
  });

  it("polls get_project on the 12 s interval and stops after unmount", async () => {
    let callCount = 0;
    mockInvoke.mockImplementation(async (...args: unknown[]): Promise<unknown> => {
      if (args[0] === "get_project") {
        callCount++;
        return makeDetail(PLAN_TASKS);
      }
      return undefined;
    });

    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    await act(async () => {
      root.render(createElement(PlanExecutionView, { projectId: "p1" }));
    });
    expect(callCount).toBe(1);

    await act(async () => {
      vi.advanceTimersByTime(12000);
      await Promise.resolve();
    });
    expect(callCount).toBe(2);

    await act(async () => {
      root.unmount();
    });
    await act(async () => {
      vi.advanceTimersByTime(12000 * 3);
      await Promise.resolve();
    });
    // Must not have polled after unmount.
    expect(callCount).toBe(2);
    container.remove();
  });

  it("renders task rows from the fetched project", async () => {
    mockInvoke.mockImplementation(async (...args: unknown[]): Promise<unknown> => {
      if (args[0] === "get_project") return makeDetail(PLAN_TASKS);
      return undefined;
    });

    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    await act(async () => {
      root.render(createElement(PlanExecutionView, { projectId: "p1" }));
    });

    expect(container.innerHTML).toContain("T1");
    expect(container.innerHTML).toContain("T3");
    expect(container.innerHTML).not.toContain("N1");

    await act(async () => {
      root.unmount();
    });
    container.remove();
  });

  it("renders 'No active plan tasks' when the project has no plan-tagged tasks", async () => {
    mockInvoke.mockImplementation(async (...args: unknown[]): Promise<unknown> => {
      if (args[0] === "get_project") return makeDetail(NON_PLAN_TASKS);
      return undefined;
    });

    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    await act(async () => {
      root.render(createElement(PlanExecutionView, { projectId: "p1" }));
    });

    expect(container.innerHTML).toContain("No active plan");

    await act(async () => {
      root.unmount();
    });
    container.remove();
  });

  it("does not poll while the document is hidden", async () => {
    let callCount = 0;
    mockInvoke.mockImplementation(async (...args: unknown[]): Promise<unknown> => {
      if (args[0] === "get_project") { callCount++; return makeDetail([]); }
      return undefined;
    });
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      get: () => "hidden",
    });

    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    await act(async () => {
      root.render(createElement(PlanExecutionView, { projectId: "p1" }));
    });
    expect(callCount).toBe(1); // mount-only

    await act(async () => {
      vi.advanceTimersByTime(12000 * 2);
      await Promise.resolve();
    });
    expect(callCount).toBe(1); // hidden ticks skipped

    await act(async () => {
      root.unmount();
    });
    container.remove();
  });

  it("stale fetch from old projectId does not clobber the new view after projectId change", async () => {
    // Simulate a slow fetch for "p-old" that resolves AFTER "p-new" has mounted.
    // The in-flight p-old result must be discarded (cancelled flag guards it).
    let resolveOldFetch!: (v: unknown) => void;
    const oldFetchPromise = new Promise((res) => { resolveOldFetch = res; });

    const OLD_TASKS: ProjectTask[] = [
      task({ id: "OLD1", status: "todo", planId: "old-plan" }),
    ];
    const NEW_TASKS: ProjectTask[] = [
      task({ id: "NEW1", status: "wip", planId: "new-plan" }),
    ];

    mockInvoke.mockImplementation(async (...args: unknown[]): Promise<unknown> => {
      if (args[0] === "get_project") {
        const { projectId: pid } = args[1] as { projectId: string };
        if (pid === "p-old") return oldFetchPromise.then(() => makeDetail(OLD_TASKS));
        return makeDetail(NEW_TASKS);
      }
      return undefined;
    });

    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    // Mount with p-old (fetch is pending — does not resolve yet).
    await act(async () => {
      root.render(createElement(PlanExecutionView, { projectId: "p-old" }));
    });

    // Switch to p-new while p-old fetch is still in-flight.
    await act(async () => {
      root.render(createElement(PlanExecutionView, { projectId: "p-new" }));
      await Promise.resolve();
    });

    // Now resolve the old fetch — must be discarded.
    await act(async () => {
      resolveOldFetch(undefined);
      await Promise.resolve();
      await Promise.resolve();
    });

    // Only NEW1 from p-new must appear; OLD1 must NOT clobber the view.
    expect(container.innerHTML).toContain("NEW1");
    expect(container.innerHTML).not.toContain("OLD1");

    await act(async () => root.unmount());
    container.remove();
  });
});

// ---- piece 3 Part B: skip/retry gating helpers ------------------------------

describe("plan control gating", () => {
  it("canRetry is true ONLY for blocked", () => {
    expect(canRetry("blocked")).toBe(true);
    for (const s of ["todo", "wip", "review", "done"] as ProjectTaskStatus[]) {
      expect(canRetry(s)).toBe(false);
    }
  });

  it("canSkip is false for done (already terminal) and wip (running mini — use Console Stop)", () => {
    expect(canSkip("done")).toBe(false);
    expect(canSkip("wip")).toBe(false);
  });

  it("canSkip is true for todo, review, blocked", () => {
    for (const s of ["todo", "review", "blocked"] as ProjectTaskStatus[]) {
      expect(canSkip(s)).toBe(true);
    }
  });
});

// ---- piece 3 Part B: skip/retry button render + invoke ----------------------

describe("PlanExecutionBody control buttons", () => {
  function mountBody(tasks: ProjectTask[], onControl: (id: string, a: "skip" | "retry") => void) {
    const prevActEnv = (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT;
    (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    act(() => {
      root.render(
        createElement(PlanExecutionBody, {
          model: buildPlanExecutionModel(tasks),
          onControl,
        }),
      );
    });
    return {
      container,
      cleanup: () => {
        act(() => root.unmount());
        container.remove();
        (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = prevActEnv;
      },
    };
  }

  function button(container: HTMLElement, label: string): HTMLButtonElement {
    const btn = [...container.querySelectorAll("button")].find((b) =>
      (b.getAttribute("aria-label") ?? "").includes(label),
    );
    if (!btn) throw new Error(`button "${label}" not found`);
    return btn as HTMLButtonElement;
  }

  it("Retry button is enabled only for a blocked task", () => {
    const onControl = vi.fn();
    const { container, cleanup } = mountBody(
      [
        task({ id: "T1", status: "blocked", planId: "p" }),
        task({ id: "T2", status: "todo", planId: "p" }),
        task({ id: "T3", status: "wip", planId: "p" }),
        task({ id: "T4", status: "done", planId: "p" }),
      ],
      onControl,
    );
    expect(button(container, "Retry T1").disabled).toBe(false); // blocked
    expect(button(container, "Retry T2").disabled).toBe(true); // todo
    expect(button(container, "Retry T3").disabled).toBe(true); // wip
    expect(button(container, "Retry T4").disabled).toBe(true); // done
    cleanup();
  });

  it("Skip button is disabled for done AND wip; enabled for todo, review, blocked", () => {
    const onControl = vi.fn();
    const { container, cleanup } = mountBody(
      [
        task({ id: "T1", status: "blocked", planId: "p" }),
        task({ id: "T2", status: "todo", planId: "p" }),
        task({ id: "T3", status: "wip", planId: "p" }),
        task({ id: "T4", status: "done", planId: "p" }),
        task({ id: "T5", status: "review", planId: "p" }),
      ],
      onControl,
    );
    expect(button(container, "Skip T1").disabled).toBe(false); // blocked → skippable
    expect(button(container, "Skip T2").disabled).toBe(false); // todo → skippable
    expect(button(container, "Skip T3").disabled).toBe(true);  // wip → backend rejects, UI gates
    expect(button(container, "Skip T4").disabled).toBe(true);  // done → already terminal
    expect(button(container, "Skip T5").disabled).toBe(false); // review → skippable
    cleanup();
  });

  it("clicking Retry calls onControl(id, 'retry')", () => {
    const onControl = vi.fn();
    const { container, cleanup } = mountBody(
      [task({ id: "T5", status: "blocked", planId: "p" })],
      onControl,
    );
    act(() => button(container, "Retry T5").click());
    expect(onControl).toHaveBeenCalledWith("T5", "retry");
    cleanup();
  });

  it("clicking Skip calls onControl(id, 'skip') for a todo task", () => {
    const onControl = vi.fn();
    const { container, cleanup } = mountBody(
      [task({ id: "T6", status: "todo", planId: "p" })],
      onControl,
    );
    act(() => button(container, "Skip T6").click());
    expect(onControl).toHaveBeenCalledWith("T6", "skip");
    cleanup();
  });

  it("renders no control buttons when onControl is omitted (read-only)", () => {
    const prevActEnv = (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT;
    (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    act(() => {
      root.render(
        createElement(PlanExecutionBody, {
          model: buildPlanExecutionModel([task({ id: "T1", status: "blocked", planId: "p" })]),
        }),
      );
    });
    expect(container.querySelectorAll("button").length).toBe(0);
    act(() => root.unmount());
    container.remove();
    (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = prevActEnv;
  });
});

// ---- piece 3 Part B: PlanExecutionView wires plan_task_control ---------------

describe("PlanExecutionView plan control wiring", () => {
  let prevActEnv: unknown;
  beforeEach(() => {
    mockInvoke.mockReset();
    prevActEnv = (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT;
    (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
  });
  afterEach(() => {
    (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = prevActEnv;
  });

  it("clicking Skip on a todo task invokes plan_task_control with the right args + the loaded revision", async () => {
    mockInvoke.mockImplementation(async (...args: unknown[]): Promise<unknown> => {
      if (args[0] === "get_project") return makeDetail(PLAN_TASKS); // revision "r1"
      if (args[0] === "plan_task_control") return makeDetail(PLAN_TASKS);
      return undefined;
    });

    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(createElement(PlanExecutionView, { projectId: "p1" }));
    });

    // T4 is todo in PLAN_TASKS — its Skip button is enabled (todo is skippable).
    const skipT4 = [...container.querySelectorAll("button")].find((b) =>
      (b.getAttribute("aria-label") ?? "").includes("Skip T4"),
    ) as HTMLButtonElement;
    expect(skipT4).toBeTruthy();
    expect(skipT4.disabled).toBe(false);

    await act(async () => {
      skipT4.click();
      await Promise.resolve();
    });

    expect(mockInvoke).toHaveBeenCalledWith("plan_task_control", {
      projectId: "p1",
      taskId: "T4",
      action: "skip",
      expectedRevision: "r1",
    });

    await act(async () => root.unmount());
    container.remove();
  });

  it("Skip button on a wip task is disabled (backend rejects skip on running tasks)", async () => {
    mockInvoke.mockImplementation(async (...args: unknown[]): Promise<unknown> => {
      if (args[0] === "get_project") return makeDetail(PLAN_TASKS);
      return undefined;
    });

    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(createElement(PlanExecutionView, { projectId: "p1" }));
    });

    // T3 is wip — Skip must be disabled; clicking must NOT call plan_task_control.
    const skipT3 = [...container.querySelectorAll("button")].find((b) =>
      (b.getAttribute("aria-label") ?? "").includes("Skip T3"),
    ) as HTMLButtonElement;
    expect(skipT3).toBeTruthy();
    expect(skipT3.disabled).toBe(true);

    // Force-click the disabled button — the guard in handleControl must still reject.
    await act(async () => {
      skipT3.click();
      await Promise.resolve();
    });

    expect(mockInvoke).not.toHaveBeenCalledWith("plan_task_control", expect.anything());

    await act(async () => root.unmount());
    container.remove();
  });

  it("clicking Retry on a blocked task invokes plan_task_control action 'retry'", async () => {
    mockInvoke.mockImplementation(async (...args: unknown[]): Promise<unknown> => {
      if (args[0] === "get_project") return makeDetail(PLAN_TASKS);
      if (args[0] === "plan_task_control") return makeDetail(PLAN_TASKS);
      return undefined;
    });

    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(createElement(PlanExecutionView, { projectId: "p1" }));
    });

    // T5 is blocked in PLAN_TASKS — its Retry button is enabled.
    const retryT5 = [...container.querySelectorAll("button")].find((b) =>
      (b.getAttribute("aria-label") ?? "").includes("Retry T5"),
    ) as HTMLButtonElement;
    expect(retryT5.disabled).toBe(false);

    await act(async () => {
      retryT5.click();
      await Promise.resolve();
    });

    expect(mockInvoke).toHaveBeenCalledWith("plan_task_control", {
      projectId: "p1",
      taskId: "T5",
      action: "retry",
      expectedRevision: "r1",
    });

    await act(async () => root.unmount());
    container.remove();
  });

  it("surfaces a control error and re-syncs via get_project on failure", async () => {
    let getProjectCalls = 0;
    mockInvoke.mockImplementation(async (...args: unknown[]): Promise<unknown> => {
      if (args[0] === "get_project") {
        getProjectCalls++;
        return makeDetail(PLAN_TASKS);
      }
      if (args[0] === "plan_task_control") {
        throw new Error("Project changed on disk. Reload before saving.");
      }
      return undefined;
    });

    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(createElement(PlanExecutionView, { projectId: "p1" }));
    });
    expect(getProjectCalls).toBe(1); // mount fetch

    const retryT5 = [...container.querySelectorAll("button")].find((b) =>
      (b.getAttribute("aria-label") ?? "").includes("Retry T5"),
    ) as HTMLButtonElement;
    await act(async () => {
      retryT5.click();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(container.textContent).toContain("Project changed on disk");
    // Failure path re-fetched to recover a fresh revision.
    expect(getProjectCalls).toBe(2);

    await act(async () => root.unmount());
    container.remove();
  });
});
