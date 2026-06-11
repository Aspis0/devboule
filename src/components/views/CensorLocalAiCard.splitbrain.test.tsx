// @vitest-environment jsdom
//
// BLOCKER 1 (split-brain config) — end-to-end TS half: the provider card
// (CensorLocalAiCard, Settings → Workspace) must NOT wipe the `ollamaModel` override
// owned by the CensorModelCard (Providers tab). It has no input for the override, but it
// reads the existing value from config and round-trips it so an Ollama "Save provider"
// preserves it. We drive a real jsdom render + Save click and assert the
// set_censor_local_ai payload still carries `ollamaModel`.

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

// Import AFTER the mock so the component binds the mocked hooks.
import { __test_CensorLocalAiCard as CensorLocalAiCard } from "./WorkspaceView";

let container: HTMLDivElement;
let root: Root;

function setNavigator(platform: string, userAgent: string) {
  Object.defineProperty(window.navigator, "platform", {
    value: platform,
    configurable: true,
  });
  Object.defineProperty(window.navigator, "userAgent", {
    value: userAgent,
    configurable: true,
  });
}

async function mount() {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () => {
    root.render(<CensorLocalAiCard />);
  });
}

beforeEach(() => {
  currentConfig = undefined;
  invokeMock.mockClear();
  refreshConfig.mockClear();
  setNavigator("Win32", "Windows");
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

describe("CensorLocalAiCard split-brain preservation", () => {
  it("preserves an existing ollamaModel when saving the Ollama provider", async () => {
    // The override was set by the CensorModelCard; the provider card must keep it.
    currentConfig = { provider: "ollama", ollamaModel: "gemma4:custom" };
    await mount();
    const saveBtn = Array.from(container.querySelectorAll("button")).find((b) =>
      b.textContent?.includes("Save provider"),
    )!;
    await act(async () => {
      saveBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    expect(invokeMock).toHaveBeenCalledTimes(1);
    const [name, args] = invokeMock.mock.calls[0];
    expect(name).toBe("set_censor_local_ai");
    const cfg = (args as { config: Record<string, unknown> }).config;
    expect(cfg.provider).toBe("ollama");
    // The override survives the provider save (no split-brain wipe).
    expect(cfg.ollamaModel).toBe("gemma4:custom");
  });

  it("saves appleFm with optional model but no stale baseUrl", async () => {
    setNavigator("MacIntel", "Mac OS X");
    currentConfig = {
      provider: "appleFm",
      baseUrl: "http://localhost:8000/v1",
      model: "mlx-community/gemma",
      other: "ignored",
    };
    await mount();
    const saveBtn = Array.from(container.querySelectorAll("button")).find((b) =>
      b.textContent?.includes("Save provider"),
    )!;
    await act(async () => {
      saveBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    expect(invokeMock).toHaveBeenCalledTimes(1);
    const cfg = (invokeMock.mock.calls[0][1] as { config: Record<string, unknown> }).config;
    expect(cfg.provider).toBe("appleFm");
    expect(cfg).toEqual({ provider: "appleFm", model: "mlx-community/gemma" });
    expect(cfg).not.toHaveProperty("baseUrl");
  });

  it("disables appleFm save on non-macOS with a clear note", async () => {
    currentConfig = { provider: "appleFm" };
    await mount();
    const saveBtn = Array.from(container.querySelectorAll("button")).find((b) =>
      b.textContent?.includes("Save provider"),
    )!;
    expect(saveBtn.disabled).toBe(true);
    expect(container.textContent).toContain("not available on this OS");
  });

  it("saves the bare default when no override exists (no churn)", async () => {
    currentConfig = undefined; // absent == ollama default, no override
    await mount();
    const saveBtn = Array.from(container.querySelectorAll("button")).find((b) =>
      b.textContent?.includes("Save provider"),
    )!;
    await act(async () => {
      saveBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    const cfg = (invokeMock.mock.calls[0][1] as { config: Record<string, unknown> })
      .config;
    expect(cfg.provider).toBe("ollama");
    expect("ollamaModel" in cfg).toBe(false);
  });
});
