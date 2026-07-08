// @vitest-environment jsdom
//
// Phase 5: the Providers & Models tab. We mock the four child cards to lightweight
// markers (their own persistence is tested in their own files) and provide a
// detect_providers IPC mock, so this test proves the WIRING: the detection strip
// renders the detected providers, and all four per-role sections mount in order.

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
vi.mock("./LocalCoderBackendCard", () => ({
  LocalCoderBackendCard: () =>
    createElement("div", { "data-testid": "local-coder-card" }),
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
vi.mock("./OracleAnswerSettingsCard", () => ({
  OracleAnswerSettingsCard: () =>
    createElement("div", { "data-testid": "oracle-llm-card" }),
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

  it("renders all per-role card sections", async () => {
    await mount();
    // Default-open surfaces render immediately: the unified Roles table, the Models group
    // (Model registry), and the Gates & helpers group (Censor + Design LLM).
    expect(
      container.querySelector('[data-testid="roles-table-card"]'),
    ).not.toBeNull();
    expect(
      container.querySelector('[data-testid="model-registry-card"]'),
    ).not.toBeNull();
    expect(
      container.querySelector('[data-testid="censor-model-card"]'),
    ).not.toBeNull();
    expect(
      container.querySelector('[data-testid="design-llm-card"]'),
    ).not.toBeNull();

    // "Coders (advanced)" is collapsed by default (the Roles table supersedes it) — expand it.
    const codersBtn = Array.from(container.querySelectorAll("button")).find((b) =>
      (b.textContent ?? "").includes("Coders (advanced)"),
    );
    await act(async () => {
      codersBtn?.click();
    });
    expect(
      container.querySelector('[data-testid="local-coder-card"]'),
    ).not.toBeNull();
    expect(
      container.querySelector('[data-testid="mini-write-behavior-card"]'),
    ).not.toBeNull();
    expect(
      container.querySelector('[data-testid="web-search-card"]'),
    ).not.toBeNull();

    // The "Oracle" group is collapsed by default — expand it, then assert.
    const oracleBtn = Array.from(container.querySelectorAll("button")).find((b) =>
      (b.textContent ?? "").includes("Oracle"),
    );
    await act(async () => {
      oracleBtn?.click();
    });
    expect(
      container.querySelector('[data-testid="oracle-llm-card"]'),
    ).not.toBeNull();
  });

  it("shows an empty-state note when nothing is detected", async () => {
    detectResult = [];
    await mount();
    expect(container.innerHTML).toContain("No local CLI or HTTP provider detected");
  });
});
