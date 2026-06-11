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
vi.mock("./CensorModelCard", () => ({
  CensorModelCard: () =>
    createElement("div", { "data-testid": "censor-model-card" }),
}));
vi.mock("./MiniCoderBackendCard", () => ({
  MiniCoderBackendCard: () =>
    createElement("div", { "data-testid": "mini-coder-card" }),
}));
vi.mock("./OracleAnswerSettingsCard", () => ({
  OracleAnswerSettingsCard: () =>
    createElement("div", { "data-testid": "oracle-llm-card" }),
}));
vi.mock("./DesignLlmBackendCard", () => ({
  DesignLlmBackendCard: () =>
    createElement("div", { "data-testid": "design-llm-card" }),
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

beforeEach(() => {
  detectResult = [];
  detectCalls = 0;
  invokeMock.mockClear();
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
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

  it("renders Apple on-device status when detected", async () => {
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

  it("renders all four per-role card sections", async () => {
    await mount();
    expect(
      container.querySelector('[data-testid="censor-model-card"]'),
    ).not.toBeNull();
    expect(
      container.querySelector('[data-testid="mini-coder-card"]'),
    ).not.toBeNull();
    expect(
      container.querySelector('[data-testid="oracle-llm-card"]'),
    ).not.toBeNull();
    expect(
      container.querySelector('[data-testid="design-llm-card"]'),
    ).not.toBeNull();
  });

  it("shows an empty-state note when nothing is detected", async () => {
    detectResult = [];
    await mount();
    expect(container.innerHTML).toContain("No local CLI or HTTP provider detected");
  });
});
