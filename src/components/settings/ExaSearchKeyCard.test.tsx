// Tests for ExaSearchKeyCard (L2.4 — local-coder Exa web-search key).
//
// Two concerns:
//  1. exaKeyBadge: the pure present/absent/error status mapping (single source of
//     truth for the badge), exhaustively over the AuxCredentialStatus shapes.
//  2. The static render: the card never renders a stored secret back (the input is
//     write-only, type=password, and starts empty), and the explainer copy is
//     present. Uses renderToStaticMarkup (no jsdom) like MiniCoderBackendCard.test.
//
// PRIVACY: get_exa_key_status returns present/absent ONLY — there is no secret in
// AuxCredentialStatus — so even a "configured" status carries no value to leak.

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

import { ExaSearchKeyCard, exaKeyBadge } from "./ExaSearchKeyCard";

beforeEach(() => {
  invokeMock.mockClear();
});

function status(over: Partial<AuxCredentialStatus>): AuxCredentialStatus {
  return {
    id: "exa_api_key",
    label: "Exa web-search API key",
    configured: false,
    status: "missing",
    lastCheckedAt: null,
    message: null,
    ...over,
  };
}

// ---------------------------------------------------------------------------
// exaKeyBadge — pure status mapping
// ---------------------------------------------------------------------------

describe("exaKeyBadge", () => {
  it("maps a null status (not loaded) to the absent/missing badge", () => {
    expect(exaKeyBadge(null)).toEqual({ tone: "missing", label: "No key" });
  });

  it("maps configured=true to the present badge", () => {
    expect(exaKeyBadge(status({ configured: true, status: "configured" }))).toEqual({
      tone: "configured",
      label: "Key saved",
    });
  });

  it("maps status=missing to the absent badge", () => {
    expect(exaKeyBadge(status({ configured: false, status: "missing" }))).toEqual({
      tone: "missing",
      label: "No key",
    });
  });

  it("maps status=error to the error badge even if it claims configured", () => {
    // Defensive: an error status must never read as 'present' regardless of the
    // configured flag (the backend sets configured=false on error, but the badge
    // does not depend on that ordering).
    expect(
      exaKeyBadge(status({ configured: true, status: "error", message: "bad" })),
    ).toEqual({ tone: "error", label: "Error" });
  });
});

// ---------------------------------------------------------------------------
// Static render — write-only, never renders the secret
// ---------------------------------------------------------------------------

describe("ExaSearchKeyCard static render", () => {
  it("renders a write-only password input that is empty (no secret echoed)", () => {
    const html = renderToStaticMarkup(<ExaSearchKeyCard />);
    expect(html).toContain('type="password"');
    // The controlled input binds the empty draft only (value="") — never a stored
    // secret. The status command returns present/absent only, so there is no value
    // to leak; assert the input renders empty and nothing non-empty is bound.
    expect(html).toContain('value=""');
    expect(html).not.toMatch(/value="[^"]+"/);
    expect(html).toContain("Exa web-search key");
  });

  it("renders the opt-in / Oracle-fallback explainer copy", () => {
    const html = renderToStaticMarkup(<ExaSearchKeyCard />);
    expect(html).toContain("Exa powers the local coder");
    expect(html).toContain("the key presence IS the switch");
    expect(html).toContain("Oracle");
  });

  it("starts in the no-key state before status loads (badge + Save, no Clear)", () => {
    const html = renderToStaticMarkup(<ExaSearchKeyCard />);
    expect(html).toContain("No key");
    // The primary button reads "Save" (not "Rotate") with no key yet.
    expect(html).toContain("Save</button>");
    expect(html).not.toContain("Rotate</button>");
    // No saved key yet -> no Clear button and no "hidden" pip line.
    expect(html).not.toContain("Clear</button>");
    expect(html).not.toContain("Confirm</button>");
    expect(html).not.toContain("(hidden)");
  });
});
