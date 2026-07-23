// @vitest-environment jsdom
//
// Tests for PolisBottomBar: oracle item in BAR_ITEMS, toggling renders OracleAskPanel.
// T1a.3 — Filters panel Complexity checkbox calls setFilter.

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

// Shared mock values so individual tests can inspect/mutate them.
const mockSetFilter = vi.fn();
const mockCityState = { externalServices: [] as unknown[] };

// useCityStore: return values from the shared mock objects.
vi.mock("../../store/cityStore", () => ({
  useCityStore: (selector: (s: unknown) => unknown) =>
    selector({
      getScanExtensions: vi.fn(),
      applyScanExtensions: vi.fn(),
      sinRecords: null,
      sinActionPending: [],
      disposeSin: vi.fn(),
      cityState: mockCityState,
      filter: { categories: [], minSeverity: null, features: [], pathGlob: "", mode: "ghost" },
      setFilter: mockSetFilter,
      resetFilter: vi.fn(),
    }),
}));

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

// ---------------------------------------------------------------------------
// Module under test — import after mocks
// ---------------------------------------------------------------------------

let PolisBottomBar: typeof import("./PolisBottomBar").PolisBottomBar;

beforeEach(async () => {
  mockSetFilter.mockClear();
  mockCityState.externalServices = [];
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
    filterSets: null,
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
        filterSets: null,
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

  it("Filters panel lists every sin rule id, including the P4 roster", () => {
    const { container } = renderBar();

    const filtersBtn = Array.from(container.querySelectorAll("button")).find(
      (b) => b.textContent?.includes("Filters"),
    )!;
    act(() => {
      filtersBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    const text = container.textContent ?? "";
    for (const label of [
      "Secrets",
      "Dep cycle",
      "TODO density",
      "Dead export",
      "Env missing",
      "Complexity",
      "God file",
      "Test gap",
      "Clones",
    ]) {
      expect(text).toContain(label);
    }
    // Panels must carry a REAL Tailwind background class — /97 does not exist
    // in the Tailwind 3.4 opacity scale and silently renders transparent.
    expect(container.innerHTML).not.toMatch(/\/97|\/8[^0-9]/);
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

  // T1a.3 — clicking the Complexity checkbox in the Filters panel calls
  // setFilter with categories containing "complexity".
  it("clicking Complexity checkbox calls setFilter with categories containing complexity", () => {
    const { container } = renderBar();

    // Open the Filters panel
    const filtersBtn = Array.from(container.querySelectorAll("button")).find(
      (b) => b.textContent?.includes("Filters"),
    )!;
    act(() => {
      filtersBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    // Find the Complexity button in the anomaly categories section
    const complexityBtn = Array.from(container.querySelectorAll("button")).find(
      (b) => b.textContent?.includes("Complexity"),
    );
    expect(complexityBtn).toBeDefined();

    act(() => {
      complexityBtn!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    // Assert setFilter was called with categories containing "complexity"
    expect(mockSetFilter).toHaveBeenCalled();
    const lastCall = mockSetFilter.mock.calls[mockSetFilter.mock.calls.length - 1][0];
    expect(lastCall.categories).toContain("complexity");
  });

  it("legend shows era monuments section without cloud harbour toggles", () => {
    const { container } = renderBar();

    const legendBtn = Array.from(container.querySelectorAll("button")).find(
      (b) => b.textContent?.includes("Legend"),
    )!;
    act(() => {
      legendBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    const text = container.textContent ?? "";
    expect(text).toContain("Era monuments");
    expect(text).not.toContain("Cloud harbour");
    expect(text).not.toContain("Cloudflare");
    expect(text).not.toContain("Scaleway");
  });

  // F69 — mapped city with 0 buildings must still expose File types so the
  // user can re-enable extensions after a filter emptied the city.
  it("empty-mapped city (0 buildings) still exposes File types, hides other panels", () => {
    const { container } = renderBar({
      buildingCount: 0,
      roadCount: 0,
      agentCount: 0,
      onFocusFile: vi.fn(),
      handleRef: makeHandleRef(),
      viewportReady: false,
      immersive: false,
      polisFocusedRef: { current: false },
      filterSets: null,
    });

    const labels = Array.from(container.querySelectorAll("button")).map(
      (b) => b.textContent ?? "",
    );
    const hasFileTypes = labels.some((t) => t.includes("File types"));
    expect(hasFileTypes).toBe(true);

    // Other panel affordances are meaningless with 0 buildings.
    for (const hidden of ["Legend", "Oracle", "Anomalies", "Filters", "Guide"]) {
      expect(labels.some((t) => t.includes(hidden))).toBe(false);
    }
  });
});

// Fix 1 — providerSwatch deterministic fallback for unknown providers.
import { providerSwatch } from "./PolisBottomBar";

describe("providerSwatch — deterministic fallback for unknown slugs", () => {
  it("known providers (cloudflare, scaleway) use PROVIDER_LIVERY", () => {
    const cf = providerSwatch("cloudflare");
    const scw = providerSwatch("scaleway");
    // Both must be valid 6-digit hex colors and NOT black (#000000).
    expect(cf).toMatch(/^#[0-9a-f]{6}$/);
    expect(scw).toMatch(/^#[0-9a-f]{6}$/);
    expect(cf).not.toBe("#000000");
    expect(scw).not.toBe("#000000");
  });

  it("two different unknown slugs produce different swatch colors", () => {
    const aws = providerSwatch("aws");
    const gcp = providerSwatch("gcp");
    expect(aws).toMatch(/^#[0-9a-f]{6}$/);
    expect(gcp).toMatch(/^#[0-9a-f]{6}$/);
    expect(aws).not.toBe(gcp);
  });

  it("same slug always produces the same color (deterministic)", () => {
    const first = providerSwatch("azure");
    const second = providerSwatch("azure");
    expect(first).toBe(second);
  });

  it("unknown slugs never produce #000000", () => {
    const swatches = ["aws", "gcp", "azure", "heroku", "digitalocean", "linode"];
    for (const slug of swatches) {
      const c = providerSwatch(slug);
      expect(c).not.toBe("#000000");
    }
  });
});
