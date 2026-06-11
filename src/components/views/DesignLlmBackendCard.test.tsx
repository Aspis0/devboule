// @vitest-environment jsdom
//
// Detection-aware behaviour tests for the Design LLM backend card. Unlike the
// mini-coder card (renderToStaticMarkup, no effects), this card runs a detect_providers
// IPC call on mount, so the tests drive a REAL render in jsdom (react-dom/client + act)
// to let the mount effect flush. The pure mapping/label/hint logic is covered separately
// in design/designProviderDetection.test.ts; here we assert the WIRING: detected
// availability is rendered, the model dropdown is populated from detection, selecting an
// unavailable CLI shows the hint, api stays selectable, and Re-detect re-queries.
//
// No testing-library dependency — a tiny react-dom/client harness matches the repo's
// existing dependency-free component-test approach (see design/useDrag.lifecycle.test.tsx).

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";

import type { AppConfig, DesignLlmBackend, DetectedProvider } from "../../types/config";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

// Mutable mock state. `detectResult` is what detect_providers resolves to; `detectCalls`
// counts invocations so we can assert mount + re-detect re-query.
let detectResult: DetectedProvider[] = [];
let detectCalls = 0;
let currentBackend: DesignLlmBackend | undefined;
const refreshConfig = vi.fn(async () => undefined);

const invokeMock = vi.fn(async (name: string, _args?: unknown) => {
  if (name === "detect_providers") {
    detectCalls += 1;
    return detectResult;
  }
  if (name === "set_design_llm_backend") return null;
  return null;
});

vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (name: string, args?: unknown) => invokeMock(name, args),
  useAppContext: () => ({
    config: { designLlmBackend: currentBackend } as AppConfig,
  }),
  useAppActions: () => ({ refreshConfig }),
}));

import { __test_DesignLlmBackendCard as DesignLlmBackendCard } from "../settings/DesignLlmBackendCard";

let container: HTMLDivElement;
let root: Root;

async function mount() {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () => {
    root.render(<DesignLlmBackendCard />);
  });
  // Flush the mount detection effect's microtasks.
  await act(async () => {
    await Promise.resolve();
  });
}

function setSelect(value: string) {
  const select = container.querySelector("select") as HTMLSelectElement;
  const setter = Object.getOwnPropertyDescriptor(
    HTMLSelectElement.prototype,
    "value",
  )!.set!;
  setter.call(select, value);
  select.dispatchEvent(new Event("change", { bubbles: true }));
}

beforeEach(() => {
  detectResult = [];
  detectCalls = 0;
  currentBackend = undefined;
  invokeMock.mockClear();
  refreshConfig.mockClear();
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

describe("DesignLlmBackendCard — detection", () => {
  it("calls detect_providers on mount and renders detected availability", async () => {
    detectResult = [
      { kind: "claude", available: true, detail: "cli only", models: [] },
      { kind: "codex", available: false, models: [] },
      { kind: "ollama", available: true, models: ["qwen2.5-coder", "llama3.1"] },
      { kind: "omlx", available: false, models: [] },
    ];
    await mount();
    expect(detectCalls).toBe(1);
    const html = container.innerHTML;
    // Claude detected; Codex not found; Ollama running with a model count.
    expect(html).toContain("Claude (subscription) — detected");
    expect(html).toContain("Codex (subscription) — not found");
    expect(html).toContain("running (2 models)");
    // api is always configurable.
    expect(html).toContain("configure a command");
  });

  it("populates the model dropdown (datalist) from detection for ollama", async () => {
    detectResult = [
      { kind: "ollama", available: true, models: ["qwen2.5-coder", "llama3.1"] },
    ];
    await mount();
    // Default kind is codex; switch to ollama to reveal the model field + datalist.
    await act(async () => setSelect("ollama"));
    const datalist = container.querySelector(
      "datalist#design-llm-detected-models",
    );
    expect(datalist).not.toBeNull();
    const options = Array.from(datalist!.querySelectorAll("option")).map(
      (o) => (o as HTMLOptionElement).value,
    );
    expect(options).toEqual(["qwen2.5-coder", "llama3.1"]);
    // The input is wired to the datalist (still free-text).
    const input = container.querySelector(
      'input[list="design-llm-detected-models"]',
    );
    expect(input).not.toBeNull();
  });

  it("shows an inline hint and hard-blocks Save for an unavailable CLI provider", async () => {
    detectResult = [{ kind: "claude", available: false, models: [] }];
    await mount();
    await act(async () => setSelect("claude"));
    expect(container.innerHTML).toContain("Claude was not found on this PC");
    const save = Array.from(container.querySelectorAll("button")).find((b) =>
      /Save backend/.test(b.textContent ?? ""),
    ) as HTMLButtonElement;
    expect(save.disabled).toBe(true);
  });

  it("keeps api selectable and saveable even when nothing is detected", async () => {
    detectResult = []; // nothing available
    await mount();
    await act(async () => setSelect("api"));
    // No hard-block hint for api.
    expect(container.innerHTML).not.toContain("was not found on this PC");
    // Type a command so the draft validates, then Save should be enabled.
    const input = container.querySelector(
      'input[placeholder="mycli chat --json"]',
    ) as HTMLInputElement;
    const setter = Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )!.set!;
    await act(async () => {
      setter.call(input, "mycli chat --json");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
    const save = Array.from(container.querySelectorAll("button")).find((b) =>
      /Save backend/.test(b.textContent ?? ""),
    ) as HTMLButtonElement;
    expect(save.disabled).toBe(false);
  });

  it("Re-detect re-queries detect_providers", async () => {
    detectResult = [{ kind: "ollama", available: false, models: [] }];
    await mount();
    expect(detectCalls).toBe(1);
    // Next detection finds ollama running with a model.
    detectResult = [{ kind: "ollama", available: true, models: ["new-model"] }];
    const redetect = Array.from(container.querySelectorAll("button")).find((b) =>
      /Re-detect/.test(b.textContent ?? ""),
    ) as HTMLButtonElement;
    await act(async () => {
      redetect.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
    });
    await act(async () => {
      await Promise.resolve();
    });
    expect(detectCalls).toBe(2);
    expect(container.innerHTML).toContain("running (1 model)");
  });

  it("survives a detection failure and still allows manual config", async () => {
    invokeMock.mockImplementationOnce(async () => {
      throw new Error("boom");
    });
    await mount();
    expect(container.innerHTML).toContain("Detection failed");
    // The selector + manual fields are still rendered.
    expect(container.querySelector("select")).not.toBeNull();
  });
});

describe("DesignLlmBackendCard — WARNING: save reconciles fresh effort/timeout (no clobber)", () => {
  it("re-fetches get_design_llm_backend on save and carries the FRESH effort, not the stale mount value", async () => {
    // Mount-time config has effort "low". The composer's model popover concurrently
    // persists effort "high" + a new timeout. On Save, the card must re-read the backend
    // and build its payload from the FRESH effort/timeout (kind/model/url from the form),
    // never clobbering the popover's write with the stale mount-time "low".
    detectResult = [{ kind: "codex", available: true, models: [] }];
    currentBackend = { kind: "codex", effort: "low", timeoutSecs: 120 };

    // get_design_llm_backend returns the UPDATED knobs (popover saved them after mount).
    const freshBackend = {
      kind: "codex",
      effort: "high",
      timeoutSecs: 300,
    } as unknown as DetectedProvider[];
    invokeMock.mockImplementation(async (name: string) => {
      if (name === "detect_providers") {
        detectCalls += 1;
        return detectResult;
      }
      if (name === "get_design_llm_backend") {
        return freshBackend;
      }
      if (name === "set_design_llm_backend") return null;
      return null;
    });

    await mount();

    const save = Array.from(container.querySelectorAll("button")).find((b) =>
      /Save backend/.test(b.textContent ?? ""),
    ) as HTMLButtonElement;
    expect(save.disabled).toBe(false);

    await act(async () => {
      save.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    // The card re-read the backend before saving…
    const getCalls = invokeMock.mock.calls.filter(
      (c) => c[0] === "get_design_llm_backend",
    );
    expect(getCalls.length).toBe(1);

    // …and the persisted payload carries the FRESH effort/timeout, not the stale ones.
    const setCall = invokeMock.mock.calls.find(
      (c) => c[0] === "set_design_llm_backend",
    );
    expect(setCall).toBeTruthy();
    const payload = (setCall![1] as { backend: DesignLlmBackend }).backend;
    expect(payload).toMatchObject({ kind: "codex", effort: "high", timeoutSecs: 300 });
  });
});
