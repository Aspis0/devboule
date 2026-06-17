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

// Default cloud-key status: absent. Tests that need a present key override this.
let cloudKeyConfigured = false;
const invokeMock = vi.fn(async (...args: unknown[]) => {
  // detect_providers returns an empty array; the save command returns the normalized value.
  if (args[0] === "detect_providers") return [];
  if (args[0] === "get_cloud_llm_key_status")
    return {
      id: "cloud_llm_api_key",
      label: "Cloud main-coder API key",
      configured: cloudKeyConfigured,
      status: cloudKeyConfigured ? "configured" : "missing",
      lastCheckedAt: null,
      message: null,
    };
  if (args[0] === "save_cloud_llm_key") {
    cloudKeyConfigured = true;
    return {
      id: "cloud_llm_api_key",
      label: "Cloud main-coder API key",
      configured: true,
      status: "configured",
      lastCheckedAt: null,
      message: null,
    };
  }
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
  cloudKeyConfigured = false;
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

  it("offers a cloud option alongside the two local kinds", () => {
    currentConfig = undefined;
    const html = renderToStaticMarkup(<LocalCoderBackendCard />);
    expect(html).toContain('value="cloud"');
  });

  it("shows the MANDATORY consent warning + the API-key input only for the cloud kind", () => {
    // Local kinds keep the "stays on your machine" disclosure and show NO consent warning.
    currentConfig = { kind: "ollama", model: "qwen2.5-coder" };
    const local = renderToStaticMarkup(<LocalCoderBackendCard />);
    expect(local).toContain("never leaves this machine");
    expect(local).not.toContain("cloud-consent-warning");
    expect(local).not.toContain("Cloud API key");

    // Cloud kind: the consent warning is present (content LEAVES the machine) + the write-only
    // API-key input is shown.
    currentConfig = { kind: "cloud", model: "openrouter/auto", baseUrl: "https://openrouter.ai/api/v1" };
    const cloud = renderToStaticMarkup(<LocalCoderBackendCard />);
    expect(cloud).toContain("cloud-consent-warning");
    expect(cloud).toContain("sends your code off this machine");
    expect(cloud).toContain("Cloud API key");
    // The cloud base URL is REQUIRED (no "(optional)" suffix) and the placeholder is the
    // OpenRouter https example.
    expect(cloud).not.toContain("Base URL (optional)");
    expect(cloud).toContain("https://openrouter.ai/api/v1");
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

  it("saves the cloud API key through save_cloud_llm_key (vault, not config.json)", async () => {
    currentConfig = { kind: "cloud", model: "openrouter/auto", baseUrl: "https://openrouter.ai/api/v1" };
    await mount();

    // Type a key into the write-only cloud key input. React tracks the controlled value via
    // its own descriptor, so set it through the native setter before dispatching `input`.
    const keyInput = Array.from(
      container.querySelectorAll('input[type="password"]'),
    )[0] as HTMLInputElement;
    expect(keyInput).toBeTruthy();
    const nativeSetter = Object.getOwnPropertyDescriptor(
      window.HTMLInputElement.prototype,
      "value",
    )!.set!;
    await act(async () => {
      nativeSetter.call(keyInput, "sk-cloud-test-key-123456");
      keyInput.dispatchEvent(new Event("input", { bubbles: true }));
    });

    const saveKeyBtn = Array.from(container.querySelectorAll("button")).find((b) =>
      b.textContent?.includes("Save key"),
    );
    expect(saveKeyBtn).toBeTruthy();
    await act(async () => {
      saveKeyBtn!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    const keyCall = invokeMock.mock.calls.find((c) => c[0] === "save_cloud_llm_key");
    expect(keyCall).toBeTruthy();
    expect(keyCall![1]).toEqual({ key: "sk-cloud-test-key-123456" });
    // The key must NEVER ride through the config-save command.
    const backendCall = invokeMock.mock.calls.find((c) => c[0] === "set_local_coder_backend");
    expect(JSON.stringify(backendCall ?? {})).not.toContain("sk-cloud-test-key-123456");
  });

  it("saves a cloud backend through set_local_coder_backend when a key is present and consent is acknowledged", async () => {
    cloudKeyConfigured = true; // a key is already saved in the vault
    currentConfig = { kind: "cloud", model: "openrouter/auto", baseUrl: "https://openrouter.ai/api/v1" };
    await mount();

    const saveBtn = Array.from(container.querySelectorAll("button")).find((b) =>
      b.textContent?.includes("Save backend"),
    );
    expect(saveBtn).toBeTruthy();
    // With a key but consent NOT yet acknowledged, Save is still disabled.
    expect((saveBtn as HTMLButtonElement).disabled).toBe(true);

    // Tick the active consent checkbox -> Save enables.
    const consent = container.querySelector(
      '[data-testid="cloud-consent-ack"]',
    ) as HTMLInputElement;
    expect(consent).toBeTruthy();
    await act(async () => {
      consent.click();
    });
    expect((saveBtn as HTMLButtonElement).disabled).toBe(false);

    await act(async () => {
      saveBtn!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    const saveCall = invokeMock.mock.calls.find((c) => c[0] === "set_local_coder_backend");
    expect(saveCall).toBeTruthy();
    expect(saveCall![1]).toEqual({
      backend: { kind: "cloud", model: "openrouter/auto", baseUrl: "https://openrouter.ai/api/v1" },
    });
  });

  it("disables Save backend for cloud when no key is saved (even with consent ticked)", async () => {
    cloudKeyConfigured = false; // no key in the vault
    currentConfig = { kind: "cloud", model: "openrouter/auto", baseUrl: "https://openrouter.ai/api/v1" };
    await mount();

    const saveBtn = Array.from(container.querySelectorAll("button")).find((b) =>
      b.textContent?.includes("Save backend"),
    );
    expect(saveBtn).toBeTruthy();
    expect((saveBtn as HTMLButtonElement).disabled).toBe(true);

    // Even after acknowledging consent, the missing key keeps Save disabled.
    const consent = container.querySelector(
      '[data-testid="cloud-consent-ack"]',
    ) as HTMLInputElement;
    expect(consent).toBeTruthy();
    await act(async () => {
      consent.click();
    });
    expect((saveBtn as HTMLButtonElement).disabled).toBe(true);
  });

  it("requires the active consent checkbox for cloud and resets it when the kind changes away", async () => {
    cloudKeyConfigured = true; // a key is present, so consent is the only remaining gate
    currentConfig = { kind: "cloud", model: "openrouter/auto", baseUrl: "https://openrouter.ai/api/v1" };
    await mount();

    const saveBtn = Array.from(container.querySelectorAll("button")).find((b) =>
      b.textContent?.includes("Save backend"),
    );
    const consent = container.querySelector(
      '[data-testid="cloud-consent-ack"]',
    ) as HTMLInputElement;
    expect(saveBtn).toBeTruthy();
    expect(consent).toBeTruthy();

    // Initially unchecked -> Save disabled.
    expect(consent.checked).toBe(false);
    expect((saveBtn as HTMLButtonElement).disabled).toBe(true);

    // Tick it -> Save enables.
    await act(async () => {
      consent.click();
    });
    expect((saveBtn as HTMLButtonElement).disabled).toBe(false);

    // Switch the backend kind away from cloud and back: the acknowledgement must reset, so
    // re-entering Cloud re-requires a fresh tick (the consent state is not sticky).
    const select = container.querySelector("select") as HTMLSelectElement;
    const nativeSelectSetter = Object.getOwnPropertyDescriptor(
      window.HTMLSelectElement.prototype,
      "value",
    )!.set!;
    await act(async () => {
      nativeSelectSetter.call(select, "ollama");
      select.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await act(async () => {
      nativeSelectSetter.call(select, "cloud");
      select.dispatchEvent(new Event("change", { bubbles: true }));
    });

    const consentAfter = container.querySelector(
      '[data-testid="cloud-consent-ack"]',
    ) as HTMLInputElement;
    expect(consentAfter).toBeTruthy();
    expect(consentAfter.checked).toBe(false);
    const saveBtnAfter = Array.from(container.querySelectorAll("button")).find((b) =>
      b.textContent?.includes("Save backend"),
    );
    expect((saveBtnAfter as HTMLButtonElement).disabled).toBe(true);
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
