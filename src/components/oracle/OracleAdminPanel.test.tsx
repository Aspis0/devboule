// @vitest-environment jsdom
//
// Phase 4(b) extraction tests for OracleAdminPanel — the Oracle ADMIN surface
// lifted out of OracleView (health strip, indexed-files browser, index
// job-progress polling). A dependency-free createRoot + act harness (matching
// the repo's other jsdom tests) drives the real component; AppContext and the
// heavy OracleDoctorPanel are mocked so no Tauri is touched.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { act, createElement, useEffect } from "react";
import { createRoot, type Root } from "react-dom/client";
import type {
  OracleDoctorReport,
  OracleIndexStatus,
  OracleIndexedFiles,
  OracleRuntime,
} from "../../types/backend";

// ---- AppContext mock ------------------------------------------------------
// A single mutable bag of context values; each test sets the slice it needs
// before rendering. All async methods resolve to no-ops so loadOraclePage and
// fetchFiles settle without touching Tauri.
type Ctx = Record<string, unknown>;
const refreshOracleIndexStatus = vi.fn(async () => undefined);
const getOracleIndexedFiles =
  vi.fn<(opts?: { limit?: number; offset?: number; filter?: string }) => Promise<OracleIndexedFiles>>(
    async () => ({ total: 0, files: [], limit: 100, offset: 0 }),
  );

let ctx: Ctx;
function baseCtx(): Ctx {
  return {
    oracleRuntime: null,
    oracleLlmSettings: null,
    oracleIndexPreferences: null,
    oracleIndexStatus: null,
    secretStatuses: [],
    refreshOracleRuntime: vi.fn(async () => undefined),
    refreshOracleLlmSettings: vi.fn(async () => undefined),
    refreshOracleIndexStatus,
    saveOracleIndexPreferences: vi.fn(async () => true),
    startOracleIndexJob: vi.fn(async () => undefined),
    startOracleIndexWatcher: vi.fn(async () => undefined),
    stopOracleIndexWatcher: vi.fn(async () => undefined),
    getOracleIndexedFiles,
    isLoading: false,
  };
}

vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: vi.fn(async () => {
    throw new Error("no runtime setup");
  }),
  useAppContext: () => ctx,
}));

// ---- OracleDoctorPanel mock ----------------------------------------------
// The real panel spawns Python; here it is a stub that, when mounted (doctor
// opened), immediately reports a full report so the "full state" health strip
// path can be asserted. The report is set by the test via `doctorCtl`.
const doctorCtl: { report: OracleDoctorReport | null } = { report: null };
vi.mock("../views/OracleDoctorPanel", () => ({
  OracleDoctorPanel: (props: {
    onReport?: (r: OracleDoctorReport) => void;
  }) => {
    // Mirror the real panel: report from an effect (post-mount), never during
    // render — so no "setState while rendering another component" warning.
    useEffect(() => {
      if (doctorCtl.report) props.onReport?.(doctorCtl.report);
    }, [props]);
    return createElement("div", { "data-testid": "doctor-panel" });
  },
}));

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let OracleAdminPanel: typeof import("./OracleAdminPanel").OracleAdminPanel;

// Every mounted root is tracked so afterEach can unmount it — otherwise the
// IndexedFilesBrowser's 250ms debounce timer (scheduled on mount) fires AFTER
// the jsdom env is torn down ("window is not defined").
const liveRoots: Root[] = [];

beforeEach(async () => {
  ctx = baseCtx();
  doctorCtl.report = null;
  refreshOracleIndexStatus.mockClear();
  getOracleIndexedFiles.mockClear();
  getOracleIndexedFiles.mockImplementation(async () => ({
    total: 0,
    files: [],
    limit: 100,
    offset: 0,
  }));
  ({ OracleAdminPanel } = await import("./OracleAdminPanel"));
});

afterEach(() => {
  // Unmount any roots still alive (cancels every pending interval/timeout) BEFORE
  // restoring real timers, so no stray timer fires post-teardown.
  for (const root of liveRoots.splice(0)) {
    act(() => root.unmount());
  }
  vi.useRealTimers();
});

async function render(): Promise<{ container: HTMLElement; root: Root }> {
  const container = document.createElement("div");
  document.body.appendChild(container);
  let root!: Root;
  await act(async () => {
    root = createRoot(container);
    root.render(createElement(OracleAdminPanel));
    // Flush the mount effects (loadOraclePage, runtime setup probe, fetchFiles).
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
  liveRoots.push(root);
  return { container, root };
}

function rerender(root: Root) {
  act(() => root.render(createElement(OracleAdminPanel)));
}

function buttons(container: HTMLElement): HTMLButtonElement[] {
  return Array.from(container.querySelectorAll("button"));
}

const runtime: OracleRuntime = {
  vectorStore: {
    backend: "lancedb",
    path: "",
    files: 0,
    records: 0,
    vectorRecords: 0,
    ready: false,
  } as OracleRuntime["vectorStore"],
  chunkStore: {
    backend: "lancedb",
    path: "",
    files: 1314,
    records: 4177,
    vectorRecords: 4177,
    ready: true,
  },
  ready: true,
  ollama: {
    cli: null,
    server: "",
    model: "",
    modelAvailable: false,
    models: [],
    message: null,
  },
  setupCommands: [],
};

describe("OracleAdminPanel — health strip", () => {
  it("renders the health strip in coarse states (no doctor loaded)", async () => {
    ctx.oracleIndexPreferences = { autoWatchOnUnlock: true, indexRoot: "/repo" };
    ctx.oracleRuntime = runtime;
    const { container } = await render();
    // The strip header + server badge render.
    expect(container.textContent).toContain("Oracle server:");
    // Coarse pass-count: runtime + live_server + workspace + index = 4 of 6
    // (embedder + provider stay neutral). The ratio text proves coarse mode.
    expect(container.textContent).toContain("4/6 checks pass");
    // The five+ doctor dots render (one per check id).
    const dots = container.querySelectorAll("span[title]");
    const ids = Array.from(dots).map((d) => d.getAttribute("title"));
    expect(ids).toContain("runtime");
    expect(ids).toContain("provider");
  });

  it("renders the health strip in full states once the doctor reports", async () => {
    ctx.oracleIndexPreferences = { autoWatchOnUnlock: true, indexRoot: "/repo" };
    ctx.oracleRuntime = runtime;
    doctorCtl.report = {
      ok: true,
      checks: [
        { id: "runtime", ok: true, detail: "", remediation: "" },
        { id: "embedder", ok: true, detail: "", remediation: "" },
        { id: "workspace", ok: true, detail: "", remediation: "" },
        { id: "index", ok: false, detail: "", remediation: "" },
        { id: "live_server", ok: true, detail: "", remediation: "" },
        { id: "provider", ok: true, detail: "", remediation: "" },
      ],
    };
    const { container, root } = await render();
    // Open the doctor (the mock reports a full 6-check report on mount).
    const runDoctor = buttons(container).find((b) =>
      b.textContent?.includes("Run doctor"),
    )!;
    await act(async () => {
      runDoctor.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
    });
    rerender(root);
    // Full report: 5 of 6 checks pass (index failed). This proves the strip is
    // now driven by the doctor report, not the coarse inference.
    expect(container.textContent).toContain("5/6 checks pass");
  });
});

describe("OracleAdminPanel — IndexedFilesBrowser", () => {
  it("paginates: Next advances the offset and refetches", async () => {
    ctx.oracleIndexStatus = {
      job: null,
      watcherRunning: false,
      index: { indexedFiles: 250, pendingFiles: 0, staleFiles: 0 },
    } as unknown as OracleIndexStatus;
    // 250 total → page size 100 → Next is enabled on page 1.
    getOracleIndexedFiles.mockImplementation(async (opts) => ({
      total: 250,
      files: [
        {
          path: `src/file-${opts?.offset ?? 0}.ts`,
          chunks: 3,
          updatedAt: "",
        },
      ],
      limit: 100,
      offset: opts?.offset ?? 0,
    }));
    const { container, root } = await render();
    // Initial fetch at offset 0.
    expect(getOracleIndexedFiles).toHaveBeenCalledWith(
      expect.objectContaining({ offset: 0, limit: 100 }),
    );
    const next = buttons(container).find((b) => b.textContent === "Next")!;
    expect(next.disabled).toBe(false);
    await act(async () => {
      next.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
      await Promise.resolve();
    });
    rerender(root);
    expect(getOracleIndexedFiles).toHaveBeenCalledWith(
      expect.objectContaining({ offset: 100 }),
    );
  });

  it("debounces the filter input before refetching", async () => {
    vi.useFakeTimers();
    ctx.oracleIndexStatus = {
      job: null,
      watcherRunning: false,
      index: { indexedFiles: 10, pendingFiles: 0, staleFiles: 0 },
    } as unknown as OracleIndexStatus;
    getOracleIndexedFiles.mockImplementation(async (opts) => ({
      total: 10,
      files: [],
      limit: 100,
      offset: opts?.offset ?? 0,
    }));
    const container = document.createElement("div");
    document.body.appendChild(container);
    let root!: Root;
    await act(async () => {
      root = createRoot(container);
      root.render(createElement(OracleAdminPanel));
    });
    // Drain mount microtasks (loadOraclePage etc.).
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    const callsAfterMount = getOracleIndexedFiles.mock.calls.length;
    const input = container.querySelector("input") as HTMLInputElement;
    const setter = Object.getOwnPropertyDescriptor(
      window.HTMLInputElement.prototype,
      "value",
    )!.set!;
    act(() => {
      setter.call(input, "worker");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
    // Before the 250ms debounce elapses, no NEW fetch with the filter.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(100);
    });
    expect(getOracleIndexedFiles.mock.calls.length).toBe(callsAfterMount);
    // After the debounce, the filtered fetch fires.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(300);
    });
    expect(getOracleIndexedFiles).toHaveBeenCalledWith(
      expect.objectContaining({ filter: "worker", offset: 0 }),
    );
    act(() => root.unmount());
  });
});

describe("OracleAdminPanel — index job-progress polling", () => {
  it("polls index status on an interval while a job is active and clears on unmount", async () => {
    vi.useFakeTimers();
    ctx.oracleIndexStatus = {
      job: { status: "running" },
      watcherRunning: false,
      index: {
        root: "/repo",
        indexedFiles: 5,
        expectedFiles: 100,
        pendingFiles: 0,
        staleFiles: 0,
      },
    } as unknown as OracleIndexStatus;
    ctx.oracleIndexPreferences = { autoWatchOnUnlock: true, indexRoot: "/repo" };

    const container = document.createElement("div");
    document.body.appendChild(container);
    let root!: Root;
    await act(async () => {
      root = createRoot(container);
      root.render(createElement(OracleAdminPanel));
    });
    // Drain mount async (loadOraclePage calls refreshOracleIndexStatus once).
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    const afterMount = refreshOracleIndexStatus.mock.calls.length;

    // The 3s interval fires a poll each tick while the job is active.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(refreshOracleIndexStatus.mock.calls.length).toBe(afterMount + 1);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(refreshOracleIndexStatus.mock.calls.length).toBe(afterMount + 2);

    // Unmount → the interval is cleared; further time advances fire no polls
    // (and no setState-after-unmount warning).
    act(() => root.unmount());
    const afterUnmount = refreshOracleIndexStatus.mock.calls.length;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10000);
    });
    expect(refreshOracleIndexStatus.mock.calls.length).toBe(afterUnmount);
  });

  it("does NOT poll when no job is active", async () => {
    vi.useFakeTimers();
    ctx.oracleIndexStatus = {
      job: { status: "idle" },
      watcherRunning: false,
      index: {
        root: "/repo",
        indexedFiles: 100,
        expectedFiles: 100,
        pendingFiles: 0,
        staleFiles: 0,
      },
    } as unknown as OracleIndexStatus;
    ctx.oracleIndexPreferences = { autoWatchOnUnlock: true, indexRoot: "/repo" };

    const container = document.createElement("div");
    document.body.appendChild(container);
    let root!: Root;
    await act(async () => {
      root = createRoot(container);
      root.render(createElement(OracleAdminPanel));
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    const afterMount = refreshOracleIndexStatus.mock.calls.length;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(15000);
    });
    // No interval polling while idle.
    expect(refreshOracleIndexStatus.mock.calls.length).toBe(afterMount);
    act(() => root.unmount());
  });
});

// ---------------------------------------------------------------------------
// Regression: fix 1 — oracleRuntime.vectorStore undefined must not crash
// ---------------------------------------------------------------------------
describe("OracleAdminPanel — vectorStore optional chain (regression)", () => {
  it("renders without crash and falls back gracefully when vectorStore is undefined", async () => {
    // Construct a runtime where vectorStore is absent (backend payload omits it).
    const runtimeNoVectorStore = {
      ...runtime,
      vectorStore: undefined,
    } as unknown as typeof runtime;

    ctx.oracleIndexPreferences = { autoWatchOnUnlock: true, indexRoot: "/repo" };
    ctx.oracleRuntime = runtimeNoVectorStore;

    // Must not throw — before the fix, accessing .vectorStore.backend crashed.
    let threw = false;
    try {
      const { container } = await render();
      // The panel must have rendered some content.
      expect(container.children.length).toBeGreaterThan(0);
    } catch {
      threw = true;
    }
    expect(threw).toBe(false);
  });

  it("falls back to chunkStore backend when vectorStore is undefined", async () => {
    const runtimeNoVectorStore = {
      ...runtime,
      vectorStore: undefined,
    } as unknown as typeof runtime;

    ctx.oracleIndexPreferences = { autoWatchOnUnlock: true, indexRoot: "/repo" };
    ctx.oracleRuntime = runtimeNoVectorStore;

    const { container } = await render();
    // The health strip backend cell should show "lancedb" (from chunkStore).
    expect(container.textContent).toContain("lancedb");
  });
});
