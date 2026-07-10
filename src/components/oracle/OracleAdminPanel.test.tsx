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
  OracleRuntimeSetup,
} from "../../types/backend";
// First-open onboarding banner and feature toggle (both exported for testing).
import { OracleRuntimeSetupBanner, OracleFeatureToggle } from "./OracleAdminPanel";

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

// Track the invokeBackendCommand mock so tests can control specific commands.
let oracleEnabled = true;
let setOracleEnabledShouldFail = false;
const invokeBackendCommandMock = vi.fn(async (...callArgs: unknown[]) => {
  const cmd = callArgs[0] as string;
  const args = callArgs[1] as Record<string, unknown> | undefined;
  if (cmd === "get_oracle_enabled") return oracleEnabled;
  if (cmd === "set_oracle_enabled") {
    if (setOracleEnabledShouldFail) throw new Error("backend error");
    oracleEnabled = (args as { enabled?: boolean })?.enabled ?? oracleEnabled;
    return true;
  }
  throw new Error("no runtime setup");
});

vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (...args: unknown[]) => invokeBackendCommandMock(...(args as [])),
  useAppContext: () => ctx,
  useAppActions: () => ({
    configureCliAgents: vi.fn(async () => ({})),
    unconfigureCliAgents: vi.fn(async () => ({})),
    cliAgentsStatus: vi.fn(async () => ({ runtimeReady: false })),
  }),
}));

// ---- OracleAnswerSettingsCard mock (mounted inside the panel) -------------
vi.mock("../settings/OracleAnswerSettingsCard", () => ({
  OracleAnswerSettingsCard: () =>
    createElement("div", { "data-testid": "oracle-llm-card" }),
}));

// ---- CliAgentsCard mock (mounted inside the panel) -----------------------
vi.mock("./CliAgentsCard", () => ({
  CliAgentsCard: () =>
    createElement("div", { "data-testid": "cli-agents-card" }),
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
  oracleEnabled = true;
  setOracleEnabledShouldFail = false;
  invokeBackendCommandMock.mockClear();
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

  it("keeps polling past the 5-min cap while the indexed count advances", async () => {
    vi.useFakeTimers();
    ctx.oracleIndexStatus = {
      job: { status: "running" },
      watcherRunning: false,
      index: {
        root: "/repo",
        indexedFiles: 100,
        expectedFiles: 2000,
        pendingFiles: 1900,
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

    // Advance ~6 minutes WHILE the count keeps advancing every 30s. The old
    // fixed 5-min cap would have stopped polling at 300000ms; the progress
    // reset must keep it alive.
    for (let i = 0; i < 12; i += 1) {
      (ctx.oracleIndexStatus as unknown as { index: { indexedFiles: number } }).index.indexedFiles += 50;
      await act(async () => {
        root.render(createElement(OracleAdminPanel));
        await vi.advanceTimersByTimeAsync(30000);
      });
    }
    // 6 minutes elapsed (> INDEX_POLL_MAX_MS); polling must still be firing.
    const before = refreshOracleIndexStatus.mock.calls.length;
    expect(before).toBeGreaterThan(afterMount);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(refreshOracleIndexStatus.mock.calls.length).toBeGreaterThan(before);

    act(() => root.unmount());
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


describe("OracleRuntimeSetupBanner — first-open onboarding", () => {
  const notReady: OracleRuntimeSetup = {
    pythonFound: true,
    pythonCommand: "python3",
    pythonVersion: "3.12.0",
    venvReady: false,
    depsReady: false,
    embedderReady: false,
    ready: false,
    embedModel: "Qwen/Qwen3-Embedding-0.6B",
    messages: [],
  };

  it("shows the required disk space and a one-click install for LanceDB + the Qwen embedder", async () => {
    const onInstall = vi.fn();
    const container = document.createElement("div");
    document.body.appendChild(container);
    let root!: Root;
    await act(async () => {
      root = createRoot(container);
      root.render(
        createElement(OracleRuntimeSetupBanner, {
          setup: notReady,
          installing: false,
          error: null,
          onInstall,
          onRetry: vi.fn(),
        }),
      );
    });
    const text = container.textContent ?? "";
    // Required disk space must be stated up front (download + installed deps).
    expect(text).toMatch(/disk/i);
    expect(text).toMatch(/GB/);
    // The two installed pieces are named so the user knows what they get.
    expect(text).toMatch(/LanceDB/);
    expect(text).toMatch(/embedder|embedding/i);
    // One-click install affordance.
    const button = container.querySelector("button");
    expect(button).not.toBeNull();
    act(() => {
      (button as HTMLButtonElement).click();
    });
    expect(onInstall).toHaveBeenCalledTimes(1);
    act(() => root.unmount());
  });
});

// ---------------------------------------------------------------------------
// OracleFeatureToggle
// ---------------------------------------------------------------------------
describe("OracleFeatureToggle", () => {
  // Render OracleFeatureToggle directly (it now lives in OracleView, not OracleAdminPanel).
  async function renderToggle(): Promise<{ container: HTMLElement; root: Root }> {
    const container = document.createElement("div");
    document.body.appendChild(container);
    let root!: Root;
    await act(async () => {
      root = createRoot(container);
      root.render(createElement(OracleFeatureToggle));
      await Promise.resolve();
      await Promise.resolve();
    });
    liveRoots.push(root);
    return { container, root };
  }

  it("renders with the initial enabled state loaded from backend", async () => {
    oracleEnabled = true;
    const { container } = await renderToggle();
    const toggle = container.querySelector('[role="switch"]') as HTMLButtonElement;
    expect(toggle).not.toBeNull();
    expect(toggle.getAttribute("aria-checked")).toBe("true");
  });

  it("renders disabled when backend reports false", async () => {
    oracleEnabled = false;
    const { container } = await renderToggle();
    const toggle = container.querySelector('[role="switch"]') as HTMLButtonElement;
    expect(toggle.getAttribute("aria-checked")).toBe("false");
  });

  it("toggles from enabled to disabled on click", async () => {
    oracleEnabled = true;
    const { container, root } = await renderToggle();
    const toggle = container.querySelector('[role="switch"]') as HTMLButtonElement;
    expect(toggle.getAttribute("aria-checked")).toBe("true");
    await act(async () => {
      toggle.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
      await Promise.resolve();
    });
    // Re-render to pick up the state change.
    act(() => root.render(createElement(OracleFeatureToggle)));
    const toggleAfter = container.querySelector('[role="switch"]') as HTMLButtonElement;
    expect(toggleAfter.getAttribute("aria-checked")).toBe("false");
    expect(invokeBackendCommandMock).toHaveBeenCalledWith("set_oracle_enabled", { enabled: false });
  });

  it("reverts to the previous state when set_oracle_enabled fails", async () => {
    oracleEnabled = true;
    setOracleEnabledShouldFail = true;
    const { container, root } = await renderToggle();
    const toggle = container.querySelector('[role="switch"]') as HTMLButtonElement;
    expect(toggle.getAttribute("aria-checked")).toBe("true");
    await act(async () => {
      toggle.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });
    // Re-render to pick up the state change.
    act(() => root.render(createElement(OracleFeatureToggle)));
    const toggleAfter = container.querySelector('[role="switch"]') as HTMLButtonElement;
    // On error, the toggle should revert to its previous state.
    expect(toggleAfter.getAttribute("aria-checked")).toBe("true");
  });
});

// ---------------------------------------------------------------------------
// CollapsibleSection rendering (Oracle LLM + CLI Agents)
// ---------------------------------------------------------------------------
describe("OracleAdminPanel — CollapsibleSections", () => {
  it("renders Oracle LLM and CLI Agents sections collapsed by default", async () => {
    const { container } = await render();
    // Both section headers should be present.
    expect(container.textContent).toContain("Oracle LLM");
    expect(container.textContent).toContain("CLI Agents");
    // By default, their children (the mocked cards) should NOT be in the DOM
    // because defaultOpen is false.
    expect(container.querySelector('[data-testid="oracle-llm-card"]')).toBeNull();
    expect(container.querySelector('[data-testid="cli-agents-card"]')).toBeNull();
  });

  it("expands Oracle LLM section when its header is clicked", async () => {
    const { container, root } = await render();
    const llmButton = Array.from(container.querySelectorAll("button")).find((b) =>
      b.textContent?.includes("Oracle LLM"),
    );
    expect(llmButton).toBeTruthy();
    await act(async () => {
      llmButton!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    rerender(root);
    // The OracleAnswerSettingsCard mock should now be visible.
    expect(container.querySelector('[data-testid="oracle-llm-card"]')).not.toBeNull();
  });

  it("expands CLI Agents section when its header is clicked", async () => {
    const { container, root } = await render();
    const cliButton = Array.from(container.querySelectorAll("button")).find((b) =>
      b.textContent?.includes("CLI Agents"),
    );
    expect(cliButton).toBeTruthy();
    await act(async () => {
      cliButton!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    rerender(root);
    // The CliAgentsCard mock should now be visible.
    expect(container.querySelector('[data-testid="cli-agents-card"]')).not.toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Stall-aware Index-now gate (TASK #4b): a job stuck in queued/running must
// not permanently lock out the "Index now" button — once the stall detector
// (indexPollStale) fires, the gate must treat jobActive as NOT active so the
// user can retry.
// ---------------------------------------------------------------------------
describe("OracleAdminPanel — stall-aware Index-now gate", () => {
  it("re-enables 'Index now' once the poll goes stale while a job is active", async () => {
    vi.useFakeTimers();
    ctx.oracleIndexPreferences = { autoWatchOnUnlock: true, indexRoot: "/repo" };
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

    const { container, root } = await render();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    const indexNow = (): HTMLButtonElement =>
      buttons(container).find((b) => b.textContent?.includes("Index now"))!;

    // Before the stall detector trips: job is active → button is disabled.
    expect(indexNow().disabled).toBe(true);

    // Advance past the 5-minute stall cap with no progress → indexPollStale fires.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(306000);
    });
    rerender(root);

    // Stale: the gate must ignore jobActive and re-enable the button for retry.
    expect(indexNow().disabled).toBe(false);
    act(() => root.unmount());
  });

  it("keeps 'Index now' disabled when a job is active and not stale", async () => {
    vi.useFakeTimers();
    ctx.oracleIndexPreferences = { autoWatchOnUnlock: true, indexRoot: "/repo" };
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

    const { container, root } = await render();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    const indexNow = (): HTMLButtonElement =>
      buttons(container).find((b) => b.textContent?.includes("Index now"))!;
    // A freshly-started job that is still progressing stays disabled.
    expect(indexNow().disabled).toBe(true);
    act(() => root.unmount());
  });

  it("does not double-fire startOracleIndexJob on rapid repeated clicks (F1 guard)", async () => {
    vi.useFakeTimers();
    ctx.oracleIndexPreferences = { autoWatchOnUnlock: true, indexRoot: "/repo" };
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

    const { container } = await render();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(306000);
    });
    const indexNow = buttons(container).find((b) =>
      b.textContent?.includes("Index now"),
    )!;
    // Stale → enabled; a slow-but-alive job can also be here, so the re-entrancy
    // guard must block a second click from starting a SECOND concurrent job.
    expect(indexNow.disabled).toBe(false);

    await act(async () => {
      // Two synchronous clicks before any microtask resolves the first call.
      indexNow.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      indexNow.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
    });
    // Only ONE index job is started despite two clicks.
    expect(ctx.startOracleIndexJob).toHaveBeenCalledTimes(1);
  });
});
