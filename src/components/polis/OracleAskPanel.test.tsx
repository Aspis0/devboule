// @vitest-environment jsdom
//
// Tests for OracleAskPanel: the Polis parchment Oracle search panel.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { OracleAnswer } from "../../types/backend";

// ---------------------------------------------------------------------------
// Shared mocks
// ---------------------------------------------------------------------------

const askOracle = vi.fn<() => Promise<OracleAnswer>>();
const requestView = vi.fn();
// Provider configured by default
const oracleLlmSettings = { apiKeyConfigured: true };
const secretStatuses: unknown[] = [];

vi.mock("../../context/AppContext", () => ({
  useAppContext: () => ({
    askOracle,
    requestView,
    oracleLlmSettings,
    secretStatuses,
  }),
}));

// cityStore: default to a null city so suggestions fall back to seedQuestions.
vi.mock("../../store/cityStore", () => ({
  useCityStore: (selector: (s: { cityState: null }) => unknown) =>
    selector({ cityState: null }),
}));

const answer: OracleAnswer = {
  mode: "llm",
  query: "test",
  answer: "The secret rotation is done by the Worker cron.",
  summary: "",
  notFound: false,
  suggestedPath: null,
  citations: [
    {
      ref: "r1",
      fileSource: "src/worker.ts",
      chunkId: "c1",
      chunkIndex: 0,
      startChar: 10,
      endChar: 42,
      retrieval: "dense",
      score: 0.9,
    },
  ],
  llmProvider: "openai",
  llmModel: "gpt-4o-mini",
  results: [],
};

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

// ---------------------------------------------------------------------------
// Import OracleAskPanel after mocks
// ---------------------------------------------------------------------------

let OracleAskPanel: typeof import("./OracleAskPanel").OracleAskPanel;

beforeEach(async () => {
  askOracle.mockClear();
  requestView.mockClear();
  ({ OracleAskPanel } = await import("./OracleAskPanel"));
});

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function renderPanel(
  onFocusFile = vi.fn(),
  onClose = vi.fn(),
): { container: HTMLElement; root: Root } {
  const container = document.createElement("div");
  document.body.appendChild(container);
  let root!: Root;
  act(() => {
    root = createRoot(container);
    root.render(createElement(OracleAskPanel, { onFocusFile, onClose }));
  });
  return { container, root };
}

// ---------------------------------------------------------------------------
// Static render tests (renderToStaticMarkup — no hooks, fast)
// ---------------------------------------------------------------------------

describe("OracleAskPanel static render", () => {
  it("renders the search input", async () => {
    const { OracleAskPanel: Panel } = await import("./OracleAskPanel");
    const html = renderToStaticMarkup(
      createElement(Panel, { onFocusFile: vi.fn(), onClose: vi.fn() }),
    );
    expect(html).toContain("<input");
  });

  it("renders suggestion chips", async () => {
    const { OracleAskPanel: Panel } = await import("./OracleAskPanel");
    const html = renderToStaticMarkup(
      createElement(Panel, { onFocusFile: vi.fn(), onClose: vi.fn() }),
    );
    // seedQuestions should appear as chips (generic, repo-agnostic)
    expect(html).toContain("What does this project do and where does it start?");
  });

  it("renders an Ask button", async () => {
    const { OracleAskPanel: Panel } = await import("./OracleAskPanel");
    const html = renderToStaticMarkup(
      createElement(Panel, { onFocusFile: vi.fn(), onClose: vi.fn() }),
    );
    expect(html).toContain("Ask");
  });

  it("has pointer-events-auto on the panel root", async () => {
    const { OracleAskPanel: Panel } = await import("./OracleAskPanel");
    const html = renderToStaticMarkup(
      createElement(Panel, { onFocusFile: vi.fn(), onClose: vi.fn() }),
    );
    expect(html).toContain("pointer-events-auto");
  });
});

// ---------------------------------------------------------------------------
// Interaction tests (createRoot — hooks run)
// ---------------------------------------------------------------------------

describe("OracleAskPanel interactions", () => {
  it("clicking Ask invokes askOracle", async () => {
    askOracle.mockResolvedValue(answer);
    const { container } = renderPanel();

    const askBtn = Array.from(container.querySelectorAll("button")).find(
      (b) => b.textContent?.trim() === "Ask",
    )!;
    expect(askBtn).toBeDefined();

    await act(async () => {
      askBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(askOracle).toHaveBeenCalled();
  });

  it("renders the answer with citation badges after a successful ask", async () => {
    askOracle.mockResolvedValue(answer);
    const { container } = renderPanel();

    const input = container.querySelector("input")!;
    // Set the input value
    act(() => {
      Object.defineProperty(input, "value", {
        configurable: true,
        get() { return "worker rotation"; },
      });
    });

    const askBtn = Array.from(container.querySelectorAll("button")).find(
      (b) => b.textContent?.trim() === "Ask",
    )!;

    await act(async () => {
      askBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    // Re-render to flush state
    act(() => {});
    expect(container.textContent).toContain("The secret rotation is done by the Worker cron.");
    expect(container.textContent).toContain("src/worker.ts:10-42");
  });

  it("clicking a citation chip calls onFocusFile with the fileSource", async () => {
    askOracle.mockResolvedValue(answer);
    const onFocusFile = vi.fn();
    const { container } = renderPanel(onFocusFile);

    const askBtn = Array.from(container.querySelectorAll("button")).find(
      (b) => b.textContent?.trim() === "Ask",
    )!;

    await act(async () => {
      askBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });
    act(() => {});

    // Find the citation chip button
    const citationBtn = Array.from(container.querySelectorAll("button")).find(
      (b) => b.textContent?.includes("src/worker.ts"),
    );
    expect(citationBtn).toBeDefined();

    act(() => {
      citationBtn!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    expect(onFocusFile).toHaveBeenCalledWith("src/worker.ts");
  });

  // BLOCKER regression: error-card admin actions (Choose folder / Run doctor) must
  // call requestView("oracle") — the standalone Oracle page with OracleAdminPanel —
  // NOT requestView("settings", "workspace") (which has no Oracle admin).
  //
  // These tests are placed BEFORE the "shows provider-not-configured hint" test that
  // calls vi.resetModules() + vi.doMock(), to avoid the module-cache / doMock-restore
  // state that makes the mock AppContext unreliable for subsequent interaction tests.
  //
  // Strategy: mock askOracle to reject with the relevant OracleError, click Ask
  // (the initial query state is seedQuestions[0] — > 3 chars — so runQuery runs),
  // flush the microtask queue until the component's catch handler runs + React
  // processes the setAskError state update, then assert the button is present and
  // clicking it calls requestView("oracle").
  it("Choose folder error action calls requestView('oracle')", async () => {
    const noWorkspaceRootError = {
      kind: "noWorkspaceRoot" as const,
      message: "No workspace root configured",
      remediation: "Pick your project root folder in the Oracle admin panel.",
    };
    askOracle.mockRejectedValue(noWorkspaceRootError);
    const { container } = renderPanel();

    const askBtn = Array.from(container.querySelectorAll("button")).find(
      (b) => b.textContent?.trim() === "Ask",
    )!;

    // Click Ask and flush the full async chain: click → runQuery → askOracle
    // rejects → setAskError → React re-render. Each await flushes one async hop.
    await act(async () => {
      askBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => { await Promise.resolve(); });
    await act(async () => { await Promise.resolve(); });
    await act(async () => { await Promise.resolve(); });

    const chooseFolderBtn = Array.from(container.querySelectorAll("button")).find(
      (b) => b.textContent?.includes("Choose folder"),
    );
    expect(chooseFolderBtn).toBeDefined();

    await act(async () => {
      chooseFolderBtn!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    expect(requestView).toHaveBeenCalledWith("oracle");
    // Must NOT be called with settings/workspace
    expect(requestView).not.toHaveBeenCalledWith("settings", "workspace");
  });

  it("Run doctor error action calls requestView('oracle')", async () => {
    const serverUnavailableError = {
      kind: "serverUnavailable" as const,
      message: "Oracle server unavailable",
      remediation: "Run the Oracle doctor to diagnose startup issues.",
    };
    askOracle.mockRejectedValue(serverUnavailableError);
    const { container } = renderPanel();

    const askBtn = Array.from(container.querySelectorAll("button")).find(
      (b) => b.textContent?.trim() === "Ask",
    )!;

    await act(async () => {
      askBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => { await Promise.resolve(); });
    await act(async () => { await Promise.resolve(); });
    await act(async () => { await Promise.resolve(); });

    const runDoctorBtn = Array.from(container.querySelectorAll("button")).find(
      (b) => b.textContent?.includes("Run doctor"),
    );
    expect(runDoctorBtn).toBeDefined();

    await act(async () => {
      runDoctorBtn!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    expect(requestView).toHaveBeenCalledWith("oracle");
    expect(requestView).not.toHaveBeenCalledWith("settings", "workspace");
  });

  it("shows provider-not-configured hint when provider is not set", async () => {
    // Re-mock AppContext to simulate no provider configured
    vi.doMock("../../context/AppContext", () => ({
      useAppContext: () => ({
        askOracle,
        requestView,
        oracleLlmSettings: null,
        secretStatuses: [],
      }),
    }));

    // We need a fresh module to pick up the new mock
    vi.resetModules();
    const { OracleAskPanel: PanelUnconfigured } = await import(
      "./OracleAskPanel"
    );
    const html = renderToStaticMarkup(
      createElement(PanelUnconfigured, { onFocusFile: vi.fn(), onClose: vi.fn() }),
    );
    expect(html).toContain("Configure Oracle provider");

    // Restore the default mock for subsequent tests
    vi.doMock("../../context/AppContext", () => ({
      useAppContext: () => ({
        askOracle,
        requestView,
        oracleLlmSettings,
        secretStatuses,
      }),
    }));
  });

  // C-F10 regression: suggestion chips must be disabled when provider not configured.
  it("suggestion chips are disabled when provider is not configured (C-F10)", async () => {
    vi.doMock("../../context/AppContext", () => ({
      useAppContext: () => ({
        askOracle,
        requestView,
        oracleLlmSettings: null,
        secretStatuses: [],
      }),
    }));
    vi.resetModules();
    const { OracleAskPanel: PanelUnconfigured } = await import("./OracleAskPanel");
    const html = renderToStaticMarkup(
      createElement(PanelUnconfigured, { onFocusFile: vi.fn(), onClose: vi.fn() }),
    );
    // All chip buttons must carry disabled (rendered as empty string in static markup).
    // We verify the disabled attribute appears on the suggestion chip buttons, which
    // means they cannot be clicked and will not silently no-op.
    expect(html).toContain("Configure Oracle provider");
    // The chips must carry disabled (static markup renders it as the attribute name).
    // Extract all button HTML: disabled chips render as `disabled=""` in SSR output.
    const chipSection = html.slice(html.indexOf("mt-2.5 flex flex-wrap"));
    expect(chipSection).toContain("disabled");

    // Restore default.
    vi.doMock("../../context/AppContext", () => ({
      useAppContext: () => ({
        askOracle,
        requestView,
        oracleLlmSettings,
        secretStatuses,
      }),
    }));
  });
});
