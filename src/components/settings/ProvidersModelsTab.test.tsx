// @vitest-environment jsdom
//
// Phase 5: the Providers & Models tab. We mock the child cards to lightweight
// markers (their own persistence is tested in their own files) and provide a
// detect_providers IPC mock, so this test proves the WIRING: the detection strip
// renders the detected providers, and the semantic sub-tabs (Models / Gates &
// helpers / Extensions / Design) switch which cards mount.
//
// Oracle LLM card was moved to OracleAdminPanel; OracleAnswerSettingsCard is no
// longer mounted here. LocalCoderBackendCard was deleted (redundant with
// RolesTableCard's Orchestrator row). PiExtensionsCard was added to Extensions.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";

import type { DetectedProvider } from "../../types/config";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let detectResult: DetectedProvider[] = [];
let detectCalls = 0;
const invokeMock = vi.fn(async (name: string) => {
  if (name === "detect_providers") {
    detectCalls += 1;
    return detectResult;
  }
  return null;
});

vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (name: string) => invokeMock(name),
}));

// Child cards mocked to markers so the tab test does not depend on their internals.
vi.mock("../views/WorkspaceView", () => ({
  CensorLocalAiCard: () =>
    createElement("div", { "data-testid": "censor-model-card" }),
}));
vi.mock("./ModelRegistryCard", () => ({
  ModelRegistryCard: () =>
    createElement("div", { "data-testid": "model-registry-card" }),
}));
// Role untangle (P6b): the unified Roles table is mocked to a marker — its own save flows
// are tested in its own file; here we only prove it mounts. (It calls useAppContext internally,
// so mocking it also keeps this tab test free of an AppContext provider.)
vi.mock("./RolesTableCard", () => ({
  RolesTableCard: () => createElement("div", { "data-testid": "roles-table-card" }),
}));
vi.mock("./MiniWriteBehaviorCard", () => ({
  MiniWriteBehaviorCard: () =>
    createElement("div", { "data-testid": "mini-write-behavior-card" }),
}));
vi.mock("./DesignLlmBackendCard", () => ({
  DesignLlmBackendCard: () =>
    createElement("div", { "data-testid": "design-llm-card" }),
}));
vi.mock("./WebSearchCard", () => ({
  WebSearchCard: () =>
    createElement("div", { "data-testid": "web-search-card" }),
}));
vi.mock("./BundledExtensionsCard", () => ({
  BundledExtensionsCard: () =>
    createElement("div", { "data-testid": "bundled-extensions-card" }),
}));
vi.mock("./UserMcpServersCard", () => ({
  UserMcpServersCard: () =>
    createElement("div", { "data-testid": "user-mcp-servers-card" }),
}));
vi.mock("../views/PiExtensionsCard", () => ({
  PiExtensionsCard: () =>
    createElement("div", { "data-testid": "pi-extensions-card" }),
}));
// RecommendedConfigCard renders for real but only uses the mocked invokeBackendCommand.
vi.mock("./RecommendedConfigCard", () => ({
  RecommendedConfigCard: () =>
    createElement("div", { "data-testid": "recommended-config-card" }),
}));

import { ProvidersModelsTab } from "./ProvidersModelsTab";

let container: HTMLDivElement;
let root: Root;

async function mount() {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () => {
    root.render(createElement(ProvidersModelsTab));
  });
  // Flush the detection mount effect.
  await act(async () => {
    await Promise.resolve();
  });
}

// Host-platform stub: the component's `isAppleHost` reads the REAL
// `navigator.platform`/`userAgent` (and jsdom embeds `process.platform` in its
// default UA), so any test asserting the per-OS clamp must pin the host
// explicitly or its outcome flips between Windows and macOS dev machines.
function stubHostPlatform(platform: string, userAgent: string) {
  Object.defineProperty(window.navigator, "platform", {
    value: platform,
    configurable: true,
  });
  Object.defineProperty(window.navigator, "userAgent", {
    value: userAgent,
    configurable: true,
  });
}

beforeEach(() => {
  detectResult = [];
  detectCalls = 0;
  invokeMock.mockClear();
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  // Drop the per-test own-property stubs so the prototype getters return.
  delete (window.navigator as { platform?: string }).platform;
  delete (window.navigator as { userAgent?: string }).userAgent;
});

describe("ProvidersModelsTab", () => {
  it("renders the detected-on-this-machine strip and calls detect_providers", async () => {
    detectResult = [
      { kind: "ollama", available: true, detail: "running", models: ["qwen", "gemma"] },
      { kind: "codex", available: true, models: [] },
    ];
    await mount();
    expect(detectCalls).toBe(1);
    const html = container.innerHTML;
    expect(html).toContain("Detected on this machine");
    // Ollama is available with 2 models → "running (2 models)" via selectorLabel.
    expect(html).toContain("Ollama");
    expect(html).toContain("2 models");
    expect(html).toContain("Codex");
  });

  it("clamps a detected Apple on-device to unavailable on a non-Apple host", async () => {
    stubHostPlatform("Win32", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)");
    detectResult = [
      {
        kind: "appleFm",
        available: true,
        detail: "configured",
        models: ["default"],
      },
    ];
    await mount();
    const html = container.innerHTML;
    expect(html).toContain("Apple on-device (local model)");
    expect(html).toContain("not available on this OS");
  });

  it("shows a detected Apple on-device as running on a macOS host", async () => {
    stubHostPlatform("MacIntel", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)");
    detectResult = [
      {
        kind: "appleFm",
        available: true,
        detail: "configured",
        models: ["default"],
      },
    ];
    await mount();
    const html = container.innerHTML;
    expect(html).toContain("Apple on-device (local model)");
    expect(html).toContain("running (1 model)");
  });

  it("shows an empty-state note when nothing is detected", async () => {
    detectResult = [];
    await mount();
    expect(container.innerHTML).toContain("No local CLI or HTTP provider detected");
  });

  describe("semantic sub-tabs", () => {
    /** Grab a sub-panel by its stable id (all four are always mounted). */
    function panel(id: string): HTMLElement | null {
      return container.querySelector(`#subtab-panel-${id}`);
    }

    it("shows the 4 sub-tab labels and has tablist/tab ARIA with one panel per tab", async () => {
      await mount();
      const tabs = Array.from(
        container.querySelectorAll('[role="tab"]'),
      ) as HTMLButtonElement[];
      const labels = tabs.map((t) => t.textContent ?? "");
      expect(labels).toContain("Models");
      expect(labels).toContain("Gates & helpers");
      expect(labels).toContain("Extensions");
      expect(labels).toContain("Design");
      expect(tabs).toHaveLength(4);
      // One tablist + four tabpanels (one per tab), each labelled by its tab.
      expect(container.querySelector('[role="tablist"]')).not.toBeNull();
      const panels = Array.from(
        container.querySelectorAll('[role="tabpanel"]'),
      ) as HTMLElement[];
      expect(panels).toHaveLength(4);
      // Every tab's aria-controls points at a panel that actually exists, and
      // every panel's aria-labelledby points back at its tab id.
      for (const tab of tabs) {
        const controls = tab.getAttribute("aria-controls")!;
        const target = container.querySelector(`#${controls}`);
        expect(target).not.toBeNull();
        expect(target?.getAttribute("role")).toBe("tabpanel");
        expect(target?.getAttribute("aria-labelledby")).toBe(tab.id);
      }
    });

    it("has Models active by default: its cards show, the other panels are mounted but hidden", async () => {
      await mount();
      const tabs = Array.from(
        container.querySelectorAll('[role="tab"]'),
      ) as HTMLButtonElement[];
      const modelsTab = tabs.find((t) => (t.textContent ?? "") === "Models")!;
      expect(modelsTab.getAttribute("aria-selected")).toBe("true");

      // Models panel is visible and its cards are in the document.
      expect(panel("models")?.hasAttribute("hidden")).toBe(false);
      expect(
        container.querySelector('[data-testid="recommended-config-card"]'),
      ).not.toBeNull();
      expect(
        container.querySelector('[data-testid="roles-table-card"]'),
      ).not.toBeNull();
      expect(
        container.querySelector('[data-testid="model-registry-card"]'),
      ).not.toBeNull();

      // The other three panels stay MOUNTED (their cards exist in the DOM) but
      // are hidden, so they are NOT visible while Models is active.
      expect(panel("gates")?.hasAttribute("hidden")).toBe(true);
      expect(panel("extensions")?.hasAttribute("hidden")).toBe(true);
      expect(panel("design")?.hasAttribute("hidden")).toBe(true);
      expect(
        container.querySelector('[data-testid="bundled-extensions-card"]'),
      ).not.toBeNull();
      expect(
        container.querySelector('[data-testid="user-mcp-servers-card"]'),
      ).not.toBeNull();
      expect(
        container.querySelector('[data-testid="pi-extensions-card"]'),
      ).not.toBeNull();
    });

    it("swaps to Extensions when its sub-tab is clicked, keeping panels mounted", async () => {
      await mount();
      const tabs = Array.from(
        container.querySelectorAll('[role="tab"]'),
      ) as HTMLButtonElement[];
      const extTab = tabs.find((t) => (t.textContent ?? "") === "Extensions")!;

      await act(async () => {
        extTab.click();
      });

      expect(extTab.getAttribute("aria-selected")).toBe("true");
      // Extensions panel now visible; Models panel hidden.
      expect(panel("extensions")?.hasAttribute("hidden")).toBe(false);
      expect(panel("models")?.hasAttribute("hidden")).toBe(true);

      // Extensions-group cards are present (mounted) and now visible.
      expect(
        container.querySelector('[data-testid="bundled-extensions-card"]'),
      ).not.toBeNull();
      expect(
        container.querySelector('[data-testid="user-mcp-servers-card"]'),
      ).not.toBeNull();
      expect(
        container.querySelector('[data-testid="pi-extensions-card"]'),
      ).not.toBeNull();

      // Models-group cards are STILL in the document (not unmounted) — they are
      // just hidden because their panel is inactive.
      expect(
        container.querySelector('[data-testid="roles-table-card"]'),
      ).not.toBeNull();
      expect(
        container.querySelector('[data-testid="model-registry-card"]'),
      ).not.toBeNull();

      // Oracle LLM card is NOT here — it moved to OracleAdminPanel.
      expect(
        container.querySelector('[data-testid="oracle-llm-card"]'),
      ).toBeNull();
    });

    it("keeps every sub-panel mounted and toggles visibility without unmounting", async () => {
      await mount();
      const tabs = Array.from(
        container.querySelectorAll('[role="tab"]'),
      ) as HTMLButtonElement[];

      // All four panels exist in the DOM initially regardless of which is active.
      for (const id of ["models", "gates", "extensions", "design"]) {
        expect(panel(id)).not.toBeNull();
      }

      // Click every tab in turn; the cards from every group stay mounted the
      // whole time (switching only toggles the `hidden` attribute).
      const order = ["Gates & helpers", "Extensions", "Design", "Models"];
      for (const label of order) {
        const tab = tabs.find((t) => (t.textContent ?? "") === label)!;
        await act(async () => {
          tab.click();
        });
        // Still all mounted after the switch.
        expect(container.querySelector('[data-testid="roles-table-card"]')).not.toBeNull();
        expect(container.querySelector('[data-testid="censor-model-card"]')).not.toBeNull();
        expect(container.querySelector('[data-testid="bundled-extensions-card"]')).not.toBeNull();
        expect(container.querySelector('[data-testid="design-llm-card"]')).not.toBeNull();
      }
      // Final state: Models active again, others hidden.
      expect(panel("models")?.hasAttribute("hidden")).toBe(false);
      expect(panel("gates")?.hasAttribute("hidden")).toBe(true);
      expect(panel("extensions")?.hasAttribute("hidden")).toBe(true);
      expect(panel("design")?.hasAttribute("hidden")).toBe(true);
    });

    it("renders each remaining sub-tab's cards and no card is dropped", async () => {
      await mount();

      async function clickSubTab(label: string) {
        const tab = Array.from(
          container.querySelectorAll('[role="tab"]'),
        ) as HTMLButtonElement[];
        const target = tab.find((t) => (t.textContent ?? "") === label)!;
        await act(async () => {
          target.click();
        });
      }

      // Gates & helpers: Censor, Mini write behavior, WebSearch.
      await clickSubTab("Gates & helpers");
      expect(panel("gates")?.hasAttribute("hidden")).toBe(false);
      expect(container.querySelector('[data-testid="censor-model-card"]')).not.toBeNull();
      expect(
        container.querySelector('[data-testid="mini-write-behavior-card"]'),
      ).not.toBeNull();
      expect(container.querySelector('[data-testid="web-search-card"]')).not.toBeNull();

      // Design: Design LLM only.
      await clickSubTab("Design");
      expect(panel("design")?.hasAttribute("hidden")).toBe(false);
      expect(container.querySelector('[data-testid="design-llm-card"]')).not.toBeNull();
      // Gates panel is now hidden (its cards remain mounted, just not visible).
      expect(panel("gates")?.hasAttribute("hidden")).toBe(true);
    });
  });
});
