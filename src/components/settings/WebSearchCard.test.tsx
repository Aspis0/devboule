// Tests for WebSearchCard (web-search settings card for the pi-web-access extension).
//
// The card shows the provider select + the SELECTED provider's key row only.
// "Auto" shows no key row (just a muted line). Switching the select does NOT
// delete keys for other providers (vault is untouched).

import { describe, expect, it, vi, beforeEach } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { AuxCredentialStatus } from "../../types/backend";

// ---------------------------------------------------------------------------
// Mocks (must precede component import)
// ---------------------------------------------------------------------------

const invokeMock = vi.fn(async () => null);

vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (...args: unknown[]) => invokeMock(...(args as [])),
}));

import { WebSearchCard, websearchKeyBadge } from "./WebSearchCard";

beforeEach(() => {
  invokeMock.mockClear();
});

function status(over: Partial<AuxCredentialStatus>): AuxCredentialStatus {
  return {
    id: "brave_api_key",
    label: "Brave web-search API key",
    configured: false,
    status: "missing",
    lastCheckedAt: null,
    message: null,
    ...over,
  };
}

// ---------------------------------------------------------------------------
// websearchKeyBadge — pure status mapping
// ---------------------------------------------------------------------------

describe("websearchKeyBadge", () => {
  it("maps a null status (not loaded) to the absent/missing badge", () => {
    expect(websearchKeyBadge(null)).toEqual({ tone: "missing", label: "No key" });
  });

  it("maps configured=true to the present badge", () => {
    expect(
      websearchKeyBadge(status({ configured: true, status: "configured" })),
    ).toEqual({ tone: "configured", label: "Key saved" });
  });

  it("maps status=missing to the absent badge", () => {
    expect(
      websearchKeyBadge(status({ configured: false, status: "missing" })),
    ).toEqual({ tone: "missing", label: "No key" });
  });

  it("maps status=error to the error badge even if it claims configured", () => {
    expect(
      websearchKeyBadge(
        status({ configured: true, status: "error", message: "bad" }),
      ),
    ).toEqual({ tone: "error", label: "Error" });
  });
});

// ---------------------------------------------------------------------------
// Static render — conditional row: 0 or 1 key row based on select
// ---------------------------------------------------------------------------

describe("WebSearchCard static render", () => {
  it("renders the select with 8 options (auto first)", () => {
    const html = renderToStaticMarkup(<WebSearchCard />);
    expect(html).toContain("Default provider");
    expect(html).toContain("Auto (recommended)");
    expect(html).toContain("Exa (no key needed)");
    expect(html).toContain("Brave");
    expect(html).toContain("Tavily");
    expect(html).toContain("Perplexity");
    expect(html).toContain("Gemini");
    expect(html).toContain("OpenAI");
    expect(html).toContain("Parallel");
  });

  it("defaults to auto: no key row, shows muted line", () => {
    const html = renderToStaticMarkup(<WebSearchCard />);
    expect(html).toContain("Automatically picks the best available provider.");
    // No password inputs when auto is selected.
    const passwordInputs = html.match(/<input[^>]*type="password"[^>]*>/g);
    expect(passwordInputs).toBeNull();
  });

  it("renders the header and subtitle copy", () => {
    const html = renderToStaticMarkup(<WebSearchCard />);
    expect(html).toContain("Web search");
    expect(html).toContain("Powered by the pi-web-access extension");
    expect(html).toContain("Works out of the box");
  });

  it("renders the privacy note about vault + env vars", () => {
    const html = renderToStaticMarkup(<WebSearchCard />);
    expect(html).toContain("Keys are stored in the app vault");
    expect(html).toContain("injected into pi sessions as env vars");
    expect(html).toContain("never touch pi");
  });

  it("starts in the no-key state before status loads (Save, no Rotate/Clear)", () => {
    const html = renderToStaticMarkup(<WebSearchCard />);
    // No password inputs (auto is default) — no Save buttons either.
    expect(html).not.toContain("Save</button>");
    expect(html).not.toContain("Rotate</button>");
    expect(html).not.toContain("Clear</button>");
  });

  it("does NOT render the Exa key-row subtitle in auto mode", () => {
    const html = renderToStaticMarkup(<WebSearchCard />);
    // The key-row note "Optional — works without a key" should NOT appear.
    expect(html).not.toContain("Optional — works without a key");
  });

  it("does NOT render any password inputs in auto mode", () => {
    const html = renderToStaticMarkup(<WebSearchCard />);
    const passwordInputs = html.match(/<input[^>]*type="password"[^>]*>/g);
    expect(passwordInputs).toBeNull();
  });

  it("help-lines mention Exa and vault", () => {
    const html = renderToStaticMarkup(<WebSearchCard />);
    expect(html).toContain("data-help-lines=");
    expect(html).toContain("Exa");
    expect(html).toContain("vault");
  });
});
