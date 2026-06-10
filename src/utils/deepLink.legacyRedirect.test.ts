// Tests for mapLegacyViewTarget (pure util).
//
// The standalone "oracle" view was RESTORED, so it is no longer remapped — it
// passes through verbatim like any other real view. This suite asserts the
// pass-through contract (and that settings#oracle is still untouched here —
// that legacy sub-tab remap is mapLegacySettingsTab's job, not this function's).

import { describe, it, expect } from "vitest";
import { mapLegacyViewTarget } from "./deepLink";

describe("mapLegacyViewTarget", () => {
  it('passes the restored "oracle" view through unchanged (no tab)', () => {
    expect(mapLegacyViewTarget("oracle")).toEqual({
      view: "oracle",
      tab: null,
    });
  });

  it('passes the restored "oracle" view through with its tab intact', () => {
    expect(mapLegacyViewTarget("oracle", "someTab")).toEqual({
      view: "oracle",
      tab: "someTab",
    });
  });

  it("passes a normal view through unchanged (no tab)", () => {
    expect(mapLegacyViewTarget("polis")).toEqual({ view: "polis", tab: null });
  });

  it("passes a normal view through with its tab intact", () => {
    expect(mapLegacyViewTarget("settings", "secrets")).toEqual({
      view: "settings",
      tab: "secrets",
    });
  });

  it('preserves settings#oracle (Oracle LLM settings tab) — NOT a standalone view redirect', () => {
    // "settings" is a real view; the tab "oracle" is the Oracle LLM settings
    // sub-tab inside Settings. This must pass through unchanged; Phase 5 will
    // rename the tab, not this function.
    expect(mapLegacyViewTarget("settings", "oracle")).toEqual({
      view: "settings",
      tab: "oracle",
    });
  });

  it("passes all other known views through unchanged", () => {
    const passThrough = [
      "dashboard",
      "providers",
      "projects",
      "cloudflare",
      "compute",
      "budget",
      "secrets",
      "devices",
      "workspace",
    ];
    for (const view of passThrough) {
      expect(mapLegacyViewTarget(view)).toEqual({ view, tab: null });
    }
  });
});
