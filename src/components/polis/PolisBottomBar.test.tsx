// @vitest-environment jsdom
//
// Tests for PolisBottomBar: oracle item in BAR_ITEMS, toggling renders OracleAskPanel.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { createRef } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";

// ---------------------------------------------------------------------------
// Mocks
// ---------------------------------------------------------------------------

// Mock OracleAskPanel so the PolisBottomBar test does not need a full AppContext.
vi.mock("./OracleAskPanel", () => ({
  OracleAskPanel: () =>
    createElement("div", { "data-testid": "oracle-ask-panel" }, "OraclePanel"),
  default: () =>
    createElement("div", { "data-testid": "oracle-ask-panel" }, "OraclePanel"),
}));

// useCityStore: return empty values (PolisBottomBar calls it via FileTypesPanel + anomalies)
vi.mock("../../store/cityStore", () => ({
  useCityStore: (selector: (s: unknown) => unknown) =>
    selector({
      getScanExtensions: vi.fn(),
      applyScanExtensions: vi.fn(),
      sinRecords: null,
      sinActionPending: [],
      disposeSin: vi.fn(),
      cityState: null,
    }),
}));

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

// ---------------------------------------------------------------------------
// Module under test — import after mocks
// ---------------------------------------------------------------------------

let PolisBottomBar: typeof import("./PolisBottomBar").PolisBottomBar;

beforeEach(async () => {
  ({ PolisBottomBar } = await import("./PolisBottomBar"));
});

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeHandleRef() {
  return createRef() as any;
}

function renderBar(
  props = {
    buildingCount: 3,
    roadCount: 2,
    agentCount: 1,
    onFocusFile: vi.fn(),
    handleRef: makeHandleRef(),
    viewportReady: false,
    immersive: false,
    polisFocusedRef: { current: false },
  },
): { container: HTMLElement; root: Root } {
  const container = document.createElement("div");
  document.body.appendChild(container);
  let root!: Root;
  act(() => {
    root = createRoot(container);
    root.render(createElement(PolisBottomBar, props));
  });
  return { container, root };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("PolisBottomBar", () => {
  it("renders the Oracle bar button", () => {
    const { container } = renderBar();
    const oracleBtn = Array.from(container.querySelectorAll("button")).find(
      (b) => b.textContent?.includes("Oracle"),
    );
    expect(oracleBtn).toBeDefined();
  });

  it("BAR_ITEMS contains an oracle entry", async () => {
    const html = renderToStaticMarkup(
      createElement(PolisBottomBar, {
        buildingCount: 1,
        roadCount: 0,
        agentCount: 0,
        handleRef: makeHandleRef(),
        viewportReady: false,
        immersive: false,
        polisFocusedRef: { current: false },
      }),
    );
    expect(html).toContain("Oracle");
  });

  it("toggling the Oracle button renders OracleAskPanel", () => {
    const { container } = renderBar();

    expect(container.querySelector("[data-testid='oracle-ask-panel']")).toBeNull();

    const oracleBtn = Array.from(container.querySelectorAll("button")).find(
      (b) => b.textContent?.includes("Oracle"),
    )!;

    act(() => {
      oracleBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    expect(
      container.querySelector("[data-testid='oracle-ask-panel']"),
    ).not.toBeNull();
  });

  it("toggling the Oracle button again closes OracleAskPanel", () => {
    const { container } = renderBar();

    const oracleBtn = Array.from(container.querySelectorAll("button")).find(
      (b) => b.textContent?.includes("Oracle"),
    )!;

    act(() => {
      oracleBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(
      container.querySelector("[data-testid='oracle-ask-panel']"),
    ).not.toBeNull();

    act(() => {
      oracleBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(container.querySelector("[data-testid='oracle-ask-panel']")).toBeNull();
  });
});
