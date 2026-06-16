// @vitest-environment jsdom
//
// Tests for LocalCoderBackendCard — the LOCAL MAIN-CODER (Devboule orchestrator) backend
// card. Proves: (1) the static shape offers ONLY the two local kinds (ollama/omlx) and is
// clearly distinct from the mini card; (2) saving persists through set_local_coder_backend
// (NOT set_mini_coder_backend) with the normalized backend.

import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { AppConfig } from "../../types/config";

// ---------------------------------------------------------------------------
// Mocks (must precede component import)
// ---------------------------------------------------------------------------

const invokeMock = vi.fn(async (...args: unknown[]) => {
  // detect_providers returns an empty array; the save command returns the normalized value.
  if (args[0] === "detect_providers") return [];
  return null;
});
const refreshMock = vi.fn(async () => undefined);
let currentConfig: AppConfig["localCoderBackend"] | undefined;

vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (...args: unknown[]) => invokeMock(...(args as [])),
  useAppContext: () => ({
    config: { localCoderBackend: currentConfig } as AppConfig,
  }),
  useAppActions: () => ({ refreshConfig: refreshMock }),
}));

import { LocalCoderBackendCard } from "./LocalCoderBackendCard";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

beforeEach(() => {
  invokeMock.mockClear();
  refreshMock.mockClear();
  currentConfig = undefined;
});

// ---------------------------------------------------------------------------
// Static shape
// ---------------------------------------------------------------------------

describe("LocalCoderBackendCard — static shape", () => {
  it("offers only the two local kinds and labels itself the MAIN coder", () => {
    currentConfig = undefined;
    const html = renderToStaticMarkup(<LocalCoderBackendCard />);
    expect(html).toContain("Local main coder (Devboule)");
    expect(html).toContain('value="ollama"');
    expect(html).toContain('value="omlx"');
    // No mini-only kinds offered (the binary can't drive them).
    expect(html).not.toContain('value="codex"');
    expect(html).not.toContain('value="api"');
    expect(html).not.toContain('value="appleFm"');
    // Model field is always present. The base URL field now shows for ollama too (the
    // default kind), but OPTIONAL — labelled as such, with the :11434 default as a
    // placeholder (editable, no hardcode lock-in).
    expect(html).toContain("Model tag");
    expect(html).toContain("Base URL (optional)");
    expect(html).toContain("http://localhost:11434/v1");
  });

  it("shows a REQUIRED base URL field when the saved kind is omlx", () => {
    currentConfig = { kind: "omlx", model: "mlx-qwen", baseUrl: "http://localhost:8000/v1" };
    const html = renderToStaticMarkup(<LocalCoderBackendCard />);
    // omlx: required, so the label has no "(optional)" suffix.
    expect(html).toContain(">Base URL");
    expect(html).not.toContain("Base URL (optional)");
    expect(html).toContain("http://localhost:8000/v1");
    expect(html).toContain("mlx-qwen");
  });

  it("prefills a saved custom ollama base URL into the editable field", () => {
    currentConfig = { kind: "ollama", model: "qwen2.5-coder", baseUrl: "http://localhost:11500/v1" };
    const html = renderToStaticMarkup(<LocalCoderBackendCard />);
    expect(html).toContain("Base URL (optional)");
    // The saved non-default URL round-trips into the input value.
    expect(html).toContain("http://localhost:11500/v1");
  });
});

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

describe("LocalCoderBackendCard — persistence", () => {
  let container: HTMLDivElement;
  let root: Root;

  async function mount() {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    await act(async () => {
      root.render(createElement(LocalCoderBackendCard));
    });
    // Flush the detect_providers mount effect.
    await act(async () => {
      await Promise.resolve();
    });
  }

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });

  it("saves an ollama backend through set_local_coder_backend (not the mini command)", async () => {
    currentConfig = { kind: "ollama", model: "qwen2.5-coder" };
    await mount();

    const saveBtn = Array.from(container.querySelectorAll("button")).find((b) =>
      b.textContent?.includes("Save backend"),
    );
    expect(saveBtn).toBeTruthy();
    await act(async () => {
      saveBtn!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    const saveCall = invokeMock.mock.calls.find(
      (c) => c[0] === "set_local_coder_backend",
    );
    expect(saveCall).toBeTruthy();
    expect(saveCall![1]).toEqual({ backend: { kind: "ollama", model: "qwen2.5-coder" } });
    // The mini command must NEVER be touched by this card.
    expect(
      invokeMock.mock.calls.some((c) => c[0] === "set_mini_coder_backend"),
    ).toBe(false);
  });

  it("persists a custom ollama base URL (Ollama on a non-default port)", async () => {
    // A saved ollama config that already carries a custom loopback URL round-trips: the
    // draft loads it and Save sends it back through set_local_coder_backend verbatim.
    currentConfig = { kind: "ollama", model: "qwen2.5-coder", baseUrl: "http://localhost:11500/v1" };
    await mount();

    const saveBtn = Array.from(container.querySelectorAll("button")).find((b) =>
      b.textContent?.includes("Save backend"),
    );
    expect(saveBtn).toBeTruthy();
    await act(async () => {
      saveBtn!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    const saveCall = invokeMock.mock.calls.find(
      (c) => c[0] === "set_local_coder_backend",
    );
    expect(saveCall).toBeTruthy();
    expect(saveCall![1]).toEqual({
      backend: { kind: "ollama", model: "qwen2.5-coder", baseUrl: "http://localhost:11500/v1" },
    });
  });
});
