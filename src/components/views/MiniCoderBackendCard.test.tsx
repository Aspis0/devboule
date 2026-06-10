import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";

import type { AppConfig, MiniCoderBackend } from "../../types/config";

// Node-env render test (this repo's vitest has no jsdom): renderToStaticMarkup
// runs the component's render path WITHOUT effects/events. We assert the static
// output — the honest disclosure copy and the kind-appropriate field — and the
// pure validation (which gates the Save button) is covered in
// agents/miniCoderBackend.test.ts. Mock AppContext so no Tauri is touched.
const invokeMock = vi.fn(async () => null);
let currentBackend: MiniCoderBackend | undefined;

vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (...args: unknown[]) => invokeMock(...(args as [])),
  useAppContext: () => ({ config: { miniCoderBackend: currentBackend } as AppConfig }),
  useAppActions: () => ({ refreshConfig: vi.fn(async () => undefined) }),
}));

// Import AFTER the mock so the component binds the mocked hooks. The card is not
// exported, so we import the WorkspaceView module and reach it via a thin probe:
// re-declare a minimal render by importing the component through a named export.
import { __test_MiniCoderBackendCard as MiniCoderBackendCard } from "../settings/MiniCoderBackendCard";

describe("MiniCoderBackendCard", () => {
  it("renders the prompt-only safety disclosure (does not oversell as a sandbox)", () => {
    currentBackend = undefined;
    const html = renderToStaticMarkup(<MiniCoderBackendCard />);
    expect(html).toContain("prompt-only safety constraint");
    expect(html).toContain("not an OS");
    expect(html).toContain("Only enable backends and repositories you trust");
  });

  it("shows the model field for the codex default backend", () => {
    currentBackend = undefined; // default kind is codex
    const html = renderToStaticMarkup(<MiniCoderBackendCard />);
    expect(html).toContain("Mini-coder backend");
    // codex shows an (optional) model field with the codex placeholder.
    expect(html).toContain("gpt-5-codex");
  });

  it("shows the command field + key disclosure when the api backend is configured", () => {
    currentBackend = { kind: "api", command: "mycli chat --json" };
    const html = renderToStaticMarkup(<MiniCoderBackendCard />);
    expect(html).toContain("Command line");
    expect(html).toContain("mycli chat --json");
    // The api hint must say the key comes from the CLI's own env, never argv.
    expect(html).toContain("never placed on");
    // A configured backend shows the Clear button.
    expect(html).toContain("Clear");
  });

  it("shows the ollama model field for an ollama backend", () => {
    currentBackend = { kind: "ollama", model: "qwen2.5-coder" };
    const html = renderToStaticMarkup(<MiniCoderBackendCard />);
    expect(html).toContain("qwen2.5-coder");
  });

  it("shows Base URL + Model and the loopback caption for an omlx backend, and hides Command", () => {
    currentBackend = {
      kind: "omlx",
      model: "qwen2.5-coder",
      baseUrl: "http://localhost:8000/v1",
    };
    const html = renderToStaticMarkup(<MiniCoderBackendCard />);
    // The omlx option is selectable in the kind picker.
    expect(html).toContain('value="omlx"');
    expect(html).toContain("oMLX (local MLX server)");
    // Base URL field with the loopback example placeholder + the seeded value.
    expect(html).toContain("Base URL");
    expect(html).toContain("http://localhost:8000/v1");
    // Model is required for omlx (reuses the model field).
    expect(html).toContain("qwen2.5-coder");
    // The loopback-only privacy caption is shown.
    expect(html).toContain("loopback only");
    // No api Command field / api trusted-shell disclosure for omlx.
    expect(html).not.toContain("Command line");
    expect(html).not.toContain("run as a shell command line with your privileges");
  });

  it("surfaces the omlx baseUrl error inline for a non-loopback origin", () => {
    // A persisted non-loopback base (should never happen via the validated Save,
    // but a config edited by hand or a stale value must show WHY Save is disabled).
    currentBackend = {
      kind: "omlx",
      model: "qwen2.5-coder",
      baseUrl: "http://evil.com/v1",
    };
    const html = renderToStaticMarkup(<MiniCoderBackendCard />);
    expect(html).toContain(
      "Base URL must be a loopback http origin",
    );
  });

  it("surfaces the omlx baseUrl error even when empty (required field)", () => {
    currentBackend = { kind: "omlx", model: "qwen2.5-coder", baseUrl: "" };
    const html = renderToStaticMarkup(<MiniCoderBackendCard />);
    expect(html).toContain("Enter the oMLX server base URL");
  });

  it("surfaces the api command error even when the command is empty (M11)", () => {
    // An api backend with an empty command must show WHY Save is disabled — an
    // inline command error — rather than just greying out the button.
    currentBackend = { kind: "api", command: "" };
    const html = renderToStaticMarkup(<MiniCoderBackendCard />);
    // The validation error for a missing api command is rendered inline.
    expect(html).toContain("Enter the CLI command line to run.");
    // FIX 8: trusted-shell-line disclosure is present — it now spells out that the
    // command runs with the user's privileges and is the same trust model as a
    // custom agent client (so the lack of metachar filtering is documented, not a bug).
    expect(html).toContain("run as a shell command line with your privileges");
    expect(html).toContain("custom agent client");
    expect(html).toContain("only configure a command you trust");
  });
});
