// @vitest-environment jsdom
//
// Phase 5: the Censor model-override card persists an Ollama model tag through
// set_censor_local_ai. We drive a real jsdom render (react-dom/client + act) so the
// Save click fires the IPC, and assert the payload carries the model ONLY under the
// dedicated `ollamaModel` field — NOT `model` (the Ollama resolver ignores `model`, and
// sending it would be dead config that could clobber a prior oMLX `model`).

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";

import type { AppConfig } from "../../types/config";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let currentConfig: unknown;
const refreshConfig = vi.fn(async () => undefined);
const invokeMock = vi.fn(async (_name: string, _args?: unknown) => null);

vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (name: string, args?: unknown) => invokeMock(name, args),
  useAppContext: () => ({ config: { censorLocalAi: currentConfig } as AppConfig }),
  useAppActions: () => ({ refreshConfig }),
}));

import { CensorModelCard } from "./CensorModelCard";

let container: HTMLDivElement;
let root: Root;

async function mount() {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () => {
    root.render(<CensorModelCard />);
  });
}

beforeEach(() => {
  currentConfig = undefined;
  invokeMock.mockClear();
  refreshConfig.mockClear();
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

describe("CensorModelCard", () => {
  it("renders the verbatim-override hint (WARNING 4) and the e2b fallback note", async () => {
    await mount();
    const html = container.innerHTML;
    expect(html).toContain("Censor model");
    expect(html).toContain("gemma4:e4b");
    expect(html).toContain("gemma4:e2b");
    // WARNING 4: the hint must state that an override is used verbatim with NO fallback,
    // and that the e2b fallback only applies to auto-select (empty override).
    expect(html).toContain("Leave empty to auto-select");
    expect(html).toContain("falling back to");
    expect(html).toContain("if only that is");
    expect(html).toContain("it is used verbatim");
    // Collapse whitespace so the JSX line-wrapping doesn't make the phrase brittle.
    expect(html.replace(/\s+/g, " ")).toContain("verbatim — no fallback");
    // The old misleading phrasing must be gone.
    expect(html).not.toContain("if e4b is");
  });

  it("seeds the input from config.censorLocalAi.ollamaModel (and legacy model)", async () => {
    currentConfig = { provider: "ollama", ollamaModel: "gemma4:e4b-custom" };
    await mount();
    const input = container.querySelector("input")!;
    expect((input as HTMLInputElement).value).toBe("gemma4:e4b-custom");
  });

  it("persists via set_censor_local_ai with ONLY ollamaModel, not model (WARNING 3)", async () => {
    await mount();
    const input = container.querySelector("input")! as HTMLInputElement;
    await act(async () => {
      const setter = Object.getOwnPropertyDescriptor(
        window.HTMLInputElement.prototype,
        "value",
      )!.set!;
      setter.call(input, "my-gemma:tag");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
    const saveBtn = Array.from(container.querySelectorAll("button")).find((b) =>
      b.textContent?.includes("Save model"),
    )!;
    await act(async () => {
      saveBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    expect(invokeMock).toHaveBeenCalledTimes(1);
    const [name, args] = invokeMock.mock.calls[0];
    expect(name).toBe("set_censor_local_ai");
    const cfg = (args as { config: Record<string, unknown> }).config;
    expect(cfg.provider).toBe("ollama");
    expect(cfg.ollamaModel).toBe("my-gemma:tag");
    // WARNING 3: `model` must NOT be sent on the Ollama branch (dead + clobbers oMLX model).
    expect("model" in cfg).toBe(false);
    expect(refreshConfig).toHaveBeenCalled();
  });

  it("clears the override (undefined ollamaModel, no model key) when saved empty", async () => {
    currentConfig = { provider: "ollama", ollamaModel: "old" };
    await mount();
    const input = container.querySelector("input")! as HTMLInputElement;
    await act(async () => {
      const setter = Object.getOwnPropertyDescriptor(
        window.HTMLInputElement.prototype,
        "value",
      )!.set!;
      setter.call(input, "");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
    const saveBtn = Array.from(container.querySelectorAll("button")).find((b) =>
      b.textContent?.includes("Save model"),
    )!;
    await act(async () => {
      saveBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    const cfg = (invokeMock.mock.calls[0][1] as { config: Record<string, unknown> })
      .config;
    expect(cfg.ollamaModel).toBeUndefined();
    expect("model" in cfg).toBe(false);
  });
});
