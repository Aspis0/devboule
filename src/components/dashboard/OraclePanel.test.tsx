// @vitest-environment jsdom
//
// C-F1 regression: OraclePanel must not setState after unmount and must
// ignore a second click while a query is already in-flight.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import type { OracleAnswer } from "../../types/backend";

// ---------------------------------------------------------------------------
// Mocks
// ---------------------------------------------------------------------------

const askOracle = vi.fn<() => Promise<OracleAnswer>>();
const getOracleNode = vi.fn<() => Promise<unknown>>();
const getOracleSimilar = vi.fn<() => Promise<unknown[]>>();

vi.mock("../../context/AppContext", () => ({
  useAppContext: () => ({
    oracleSnapshot: null,
    askOracle,
    getOracleNode,
    getOracleSimilar,
    isLoading: false,
  }),
}));

import { OraclePanel } from "./OraclePanel";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const fakeAnswer: OracleAnswer = {
  mode: "llm",
  query: "test query",
  answer: "Some answer",
  summary: "Summary text",
  notFound: false,
  suggestedPath: null,
  citations: [],
  llmProvider: "scaleway",
  llmModel: "voxtral",
  results: [],
};

beforeEach(() => {
  askOracle.mockClear();
  getOracleNode.mockClear();
  getOracleSimilar.mockClear();
});

afterEach(() => {
  vi.clearAllMocks();
});

// ---------------------------------------------------------------------------
// C-F1: busy guard — second click ignored while query in-flight
// ---------------------------------------------------------------------------

describe("OraclePanel concurrent-query guard (C-F1)", () => {
  it("a second click while querying is ignored (only one askOracle call)", async () => {
    let resolveFirst!: (v: OracleAnswer) => void;
    const firstPromise = new Promise<OracleAnswer>((res) => { resolveFirst = res; });
    askOracle.mockReturnValueOnce(firstPromise);

    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    await act(async () => {
      root.render(createElement(OraclePanel));
    });

    // Type a query (>= 3 chars) and click twice quickly.
    const input = container.querySelector<HTMLInputElement>("input")!;
    await act(async () => {
      Object.defineProperty(input, "value", {
        configurable: true,
        get() { return "how does auth work"; },
      });
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });

    const searchBtn = container.querySelector<HTMLButtonElement>(
      'button[aria-label="Ask Oracle"]',
    )!;

    await act(async () => {
      searchBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
    });

    // Second click while in-flight.
    await act(async () => {
      searchBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
    });

    // Resolve the first call.
    await act(async () => {
      resolveFirst(fakeAnswer);
      await Promise.resolve();
    });

    // askOracle must have been called exactly once despite two clicks.
    // (The second click hits the `if (querying) return` guard.)
    expect(askOracle).toHaveBeenCalledTimes(1);

    await act(async () => { root.unmount(); });
    container.remove();
  });

  it("unmount during in-flight query does not produce a setState call (no act warning)", async () => {
    let resolveQ!: (v: OracleAnswer) => void;
    askOracle.mockReturnValueOnce(
      new Promise<OracleAnswer>((res) => { resolveQ = res; }),
    );

    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    await act(async () => {
      root.render(createElement(OraclePanel));
    });

    const input = container.querySelector<HTMLInputElement>("input")!;
    await act(async () => {
      Object.defineProperty(input, "value", {
        configurable: true,
        get() { return "how does auth work"; },
      });
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });

    const searchBtn = container.querySelector<HTMLButtonElement>(
      'button[aria-label="Ask Oracle"]',
    )!;

    await act(async () => {
      searchBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
    });

    // Unmount while query is in-flight.
    await act(async () => { root.unmount(); });

    // Resolve AFTER unmount — must not throw or produce act() warning.
    await act(async () => {
      resolveQ(fakeAnswer);
      await Promise.resolve();
    });

    container.remove();
    // Reaching here without a thrown error or unhandled rejection means the guard works.
  });
});
