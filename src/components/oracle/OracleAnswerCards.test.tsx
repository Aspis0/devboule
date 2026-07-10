// @vitest-environment jsdom
//
// TASK #10 — OracleAnswerCards indexing-awareness.
//
// When Oracle is actively INDEXING (a healthy, transient state) the
// transient-availability errors (serverUnavailable / embedderUnavailable /
// pythonError) must render a calm "Indexing…" message instead of a hard
// "unavailable / not responding" error. This test drives AskErrorCard with a
// mocked AppContext and asserts both the indexing and genuinely-down paths.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { OracleError, OracleIndexStatus } from "../../types/backend";
import { AskErrorCard } from "./OracleAnswerCards";

// ---- AppContext mock ------------------------------------------------------
// A single mutable bag; each test sets oracleIndexStatus before rendering.
type AppCtx = Record<string, unknown>;
let ctx: AppCtx;

vi.mock("../../context/AppContext", () => ({
  useAppContext: () => ctx,
}));

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const liveRoots: Root[] = [];

beforeEach(() => {
  ctx = { oracleIndexStatus: null };
  for (const root of liveRoots.splice(0)) {
    act(() => root.unmount());
  }
});

function renderCard(error: OracleError): HTMLElement {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  act(() => {
    root.render(
      createElement(AskErrorCard, {
        error,
        onChooseFolder: () => undefined,
        onRunDoctor: () => undefined,
        onConfigureProvider: () => undefined,
      }),
    );
  });
  liveRoots.push(root);
  return container;
}

function indexingStatus(): OracleIndexStatus {
  return {
    job: {
      jobId: "job-1",
      startedAt: "2026-07-10T00:00:00Z",
      phase: "embedding",
    },
    watcherRunning: true,
    index: {
      root: "/work",
      expectedFiles: 10,
      indexedFiles: 0,
      pendingFiles: 10,
      staleFiles: 0,
      sqliteChunkFiles: 0,
      sqliteChunks: 0,
      vectorRecords: 0,
      firstPending: ["/work/a.ts"],
      firstStale: [],
      freeRamGb: 8,
    },
  };
}

const transientError: OracleError = {
  kind: "serverUnavailable",
  message: "Oracle server is not responding",
  remediation: "Start the Oracle Python server.",
};

describe("AskErrorCard — indexing awareness", () => {
  it("shows the calm indexing message when indexing is active", () => {
    ctx.oracleIndexStatus = indexingStatus();
    const container = renderCard(transientError);
    expect(container.textContent).toContain("Oracle is indexing your workspace");
    expect(container.textContent).toContain(
      "It's building the search index right now",
    );
    // The hard "unavailable" copy must NOT be shown while indexing.
    expect(container.textContent).not.toContain(
      "Oracle server is not responding",
    );
  });

  it("keeps the original hard error copy when not indexing", () => {
    ctx.oracleIndexStatus = null;
    const container = renderCard(transientError);
    expect(container.textContent).toContain("Oracle server is not responding");
    expect(container.textContent).not.toContain(
      "Oracle is indexing your workspace",
    );
  });

  it("detects indexing via pendingFiles even when job is null", () => {
    ctx.oracleIndexStatus = {
      ...indexingStatus(),
      job: null,
    };
    const container = renderCard(transientError);
    expect(container.textContent).toContain("Oracle is indexing your workspace");
    expect(container.textContent).not.toContain(
      "Oracle server is not responding",
    );
  });

  it("mentions indexing-in-progress for indexEmpty while indexing", () => {
    ctx.oracleIndexStatus = indexingStatus();
    const container = renderCard({
      kind: "indexEmpty",
      message: "The Oracle index is empty",
      remediation: "Index your workspace.",
    });
    expect(container.textContent).toContain(
      "Indexing may still be in progress",
    );
  });
});
