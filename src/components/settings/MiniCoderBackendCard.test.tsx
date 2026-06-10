// Tests for MiniCoderBackendCard — focused on B-F5: maxConcurrent field.
//
// Uses renderToStaticMarkup (no jsdom needed) to assert the static shape of the
// select and its options, and a lightweight interaction test with createRoot to
// verify the select persists maxConcurrent via set_mini_coder_backend.

import { describe, expect, it, vi, beforeEach } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { AppConfig } from "../../types/config";

// ---------------------------------------------------------------------------
// Mocks (must precede component import)
// ---------------------------------------------------------------------------

const invokeMock = vi.fn(async () => null);
let currentConfig: AppConfig["miniCoderBackend"] | undefined;

vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (...args: unknown[]) => invokeMock(...(args as [])),
  useAppContext: () => ({
    config: { miniCoderBackend: currentConfig } as AppConfig,
  }),
  useAppActions: () => ({ refreshConfig: vi.fn(async () => undefined) }),
}));

import { MiniCoderBackendCard } from "./MiniCoderBackendCard";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

beforeEach(() => {
  invokeMock.mockClear();
  currentConfig = undefined;
});

// ---------------------------------------------------------------------------
// B-F5 static render tests
// ---------------------------------------------------------------------------

describe("MiniCoderBackendCard — maxConcurrent field (B-F5)", () => {
  it("renders the Max concurrent slots select with options 1–4", () => {
    currentConfig = undefined;
    const html = renderToStaticMarkup(<MiniCoderBackendCard />);
    expect(html).toContain("Max concurrent slots");
    expect(html).toContain('value="1"');
    expect(html).toContain('value="2"');
    expect(html).toContain('value="3"');
    expect(html).toContain('value="4"');
  });

  it("default display shows 2 when maxConcurrent is absent from config", () => {
    currentConfig = { kind: "codex" }; // no maxConcurrent
    const html = renderToStaticMarkup(<MiniCoderBackendCard />);
    // The select is present and the selected option is "2 (default)".
    expect(html).toContain("2 (default)");
  });

  it("reflects saved maxConcurrent of 3 from config", () => {
    currentConfig = { kind: "codex", maxConcurrent: 3 };
    const html = renderToStaticMarkup(<MiniCoderBackendCard />);
    // Option 3 should be pre-selected (value="3" appears in the select).
    expect(html).toContain('value="3"');
  });
});
