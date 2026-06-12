import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";

import type { AppConfig, CensorLocalAi } from "../../types/config";

// Node-env render test (this repo's vitest has no jsdom): renderToStaticMarkup runs
// the component's render path WITHOUT effects/events. We assert the static output —
// the provider select, the oMLX-only Base URL + Model fields, the loopback caption,
// and the inline validation error for a bad base — while the pure validation that
// gates Save is covered in projects/censorLocalAi.test.ts. Mock AppContext so no
// Tauri is touched.
const invokeMock = vi.fn(async () => null);
let currentConfig: CensorLocalAi | undefined;

vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (...args: unknown[]) => invokeMock(...(args as [])),
  useAppContext: () => ({ config: { censorLocalAi: currentConfig } as AppConfig }),
  useAppActions: () => ({ refreshConfig: vi.fn(async () => undefined) }),
}));

// Import AFTER the mock so the component binds the mocked hooks.
import { __test_CensorLocalAiCard as CensorLocalAiCard } from "./WorkspaceView";

describe("CensorLocalAiCard", () => {
  it("renders the provider select with both options (Ollama default + oMLX)", () => {
    currentConfig = undefined; // absent == ollama default
    const html = renderToStaticMarkup(<CensorLocalAiCard />);
    expect(html).toContain("Censor local AI");
    expect(html).toContain('value="ollama"');
    expect(html).toContain("Ollama (default)");
    expect(html).toContain('value="appleFm"');
    expect(html).toContain("Apple on-device");
    expect(html).toContain('value="omlx"');
    expect(html).toContain("oMLX (local MLX server)");
  });

  it("renders Apple on-device as optional model-only config with no base URL field", () => {
    currentConfig = { provider: "appleFm", model: "apple-default" };
    const html = renderToStaticMarkup(<CensorLocalAiCard />);
    expect(html).toContain("Model (optional)");
    expect(html).toContain("apple-default");
    expect(html).not.toContain("Base URL");
  });

  it("hides the oMLX Base URL + Model fields when the provider is Ollama", () => {
    currentConfig = undefined; // ollama default
    const html = renderToStaticMarkup(<CensorLocalAiCard />);
    // The omlx-only fields and caption are not rendered for ollama. "Local
    // OpenAI-compatible endpoint" is the distinctive omlx caption opener (the
    // help-lines attribute uses different wording), so it must be absent here.
    expect(html).not.toContain("Base URL");
    expect(html).not.toContain("mlx-community/gemma");
    expect(html).not.toContain("Local OpenAI-compatible endpoint");
    // Ollama shows the model-tag override input instead (merged from the old
    // CensorModelCard, 2026-06-12).
    expect(html).toContain("Ollama model tag (optional)");
    expect(html).toContain("gemma4:e4b");
  });

  it("shows Base URL + Model and the loopback caption for an omlx config (no API-key field)", () => {
    currentConfig = {
      provider: "omlx",
      baseUrl: "http://localhost:8000/v1",
      model: "mlx-community/gemma",
    };
    const html = renderToStaticMarkup(<CensorLocalAiCard />);
    expect(html).toContain("Base URL");
    expect(html).toContain("http://localhost:8000/v1");
    expect(html).toContain("Model");
    expect(html).toContain("mlx-community/gemma");
    // The loopback-only privacy caption is shown.
    expect(html).toContain("loopback only");
    // No API-key field (loopback-only, like Ollama).
    expect(html).toContain("No API");
    expect(html.toLowerCase()).not.toContain('type="password"');
  });

  it("surfaces the omlx baseUrl error inline for a non-loopback origin (Save blocked)", () => {
    // A persisted non-loopback base (hand-edited / stale): the card must show WHY Save
    // is disabled rather than just greying out the button.
    currentConfig = {
      provider: "omlx",
      baseUrl: "http://evil.com/v1",
      model: "mlx-community/gemma",
    };
    const html = renderToStaticMarkup(<CensorLocalAiCard />);
    expect(html).toContain("Base URL must be a loopback http origin");
    // Save is disabled while the base is invalid: the Save button's opening tag
    // (which carries the `disabled` boolean attribute) precedes its "Save provider"
    // label, so a disabled<...>Save provider match proves the button is gated.
    expect(html).toMatch(/<button[^>]*\bdisabled\b[^>]*>[\s\S]*?Save provider/);
  });

  it("surfaces the omlx baseUrl + model errors even when empty (required fields)", () => {
    currentConfig = { provider: "omlx", baseUrl: "", model: "" };
    const html = renderToStaticMarkup(<CensorLocalAiCard />);
    expect(html).toContain("Enter the oMLX server base URL");
    expect(html).toContain("Enter the oMLX model id");
  });

  it("clamps a hand-edited bogus provider to the Ollama default (no indeterminate select)", () => {
    // max-recall FIX 5: the card seeds from the UNTYPED config passthrough. A bogus
    // provider must clamp to "ollama" (default behavior) rather than seeding an
    // indeterminate <select>, so the Ollama branch renders.
    currentConfig = { provider: "bogus" } as unknown as CensorLocalAi;
    const html = renderToStaticMarkup(<CensorLocalAiCard />);
    // Ollama branch renders (its tag input), and NO omlx-only fields leak in.
    expect(html).toContain("Ollama model tag (optional)");
    expect(html).not.toContain("Local OpenAI-compatible endpoint");
  });

  it("coerces a non-string baseUrl/model to '' without crashing (hand-edited config)", () => {
    // max-recall FIX 5: a non-string baseUrl/model (e.g. a number) must not crash the
    // controlled input / the validator's .trim(); it is coerced to "". With provider
    // omlx and empty (coerced) fields, the required-field errors are shown.
    currentConfig = {
      provider: "omlx",
      baseUrl: 1234,
      model: 5678,
    } as unknown as CensorLocalAi;
    const html = renderToStaticMarkup(<CensorLocalAiCard />);
    expect(html).toContain("Enter the oMLX server base URL");
    expect(html).toContain("Enter the oMLX model id");
    // The numeric values must NOT have rendered into the inputs.
    expect(html).not.toContain("1234");
    expect(html).not.toContain("5678");
  });
});
