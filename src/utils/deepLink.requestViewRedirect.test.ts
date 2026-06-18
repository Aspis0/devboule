// @vitest-environment node
//
// requestView("oracle") must reach the RESTORED standalone oracle view — i.e.
// mapLegacyViewTarget no longer redirects it. We test the pure
// mapLegacyViewTarget function here (its integration with requestView is in
// AppContext, which applies the mapping verbatim).

import { describe, it, expect } from "vitest";
import { mapLegacyViewTarget } from "./deepLink";

describe("requestView oracle pass-through (via mapLegacyViewTarget)", () => {
  it('requestView("oracle") input passes through to { view:"oracle", tab:null }', () => {
    const { view, tab } = mapLegacyViewTarget("oracle");
    expect(view).toBe("oracle");
    expect(tab).toBe(null);
  });

  it('requestView("oracle", null) also passes through unchanged', () => {
    const { view, tab } = mapLegacyViewTarget("oracle", null);
    expect(view).toBe("oracle");
    expect(tab).toBe(null);
  });

  it("a non-oracle view passes through to the real requestView unchanged", () => {
    // Covers: projects, polis, providers (the removed "dashboard" now redirects
    // → asserted in deepLink.legacyRedirect.test.ts, not here).
    for (const v of ["projects", "polis", "providers"]) {
      const result = mapLegacyViewTarget(v);
      expect(result.view).toBe(v);
    }
  });

  it("settings#oracle tab passes through unchanged (oracle LLM config tab)", () => {
    const result = mapLegacyViewTarget("settings", "oracle");
    expect(result).toEqual({ view: "settings", tab: "oracle" });
  });
});
