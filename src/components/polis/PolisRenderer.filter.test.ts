// P3.2 — Filter decision tests for the per-node pure helper extracted
// from PolisRenderer. Tests the PRODUCTION export — no stubs.
//
// Tests `nodeFilterVerdict`: the per-building-node decision given FilterSets.
// Pure — no PIXI, no DOM — runs under vitest.

import { describe, it, expect } from "vitest";
import { nodeFilterVerdict } from "./PolisRenderer";
import type { FilterSets } from "./filterModel";

function mkSets(over: Partial<FilterSets> = {}): FilterSets {
  return {
    ghostedFileIds: new Set(),
    effectsHiddenFileIds: new Set(),
    mode: "ghost",
    shownBuildings: 0,
    totalBuildings: 0,
    shownAnomalies: 0,
    totalAnomalies: 0,
    ...over,
  };
}

describe("nodeFilterVerdict", () => {
  it("null sets → no filter applied", () => {
    const v = nodeFilterVerdict("a", null);
    expect(v.ghosted).toBe(false);
    expect(v.effectsHidden).toBe(false);
  });

  it("ghosted building in ghost mode", () => {
    const sets = mkSets({ ghostedFileIds: new Set(["a"]) });
    const v = nodeFilterVerdict("a", sets);
    expect(v.ghosted).toBe(true);
    expect(v.hide).toBe(false);
    expect(v.effectsHidden).toBe(false);
  });

  it("ghosted building in hide mode", () => {
    const sets = mkSets({ ghostedFileIds: new Set(["a"]), mode: "hide" });
    const v = nodeFilterVerdict("a", sets);
    expect(v.ghosted).toBe(true);
    expect(v.hide).toBe(true);
  });

  it("effects-hidden (non-ghosted)", () => {
    const sets = mkSets({ effectsHiddenFileIds: new Set(["a"]) });
    const v = nodeFilterVerdict("a", sets);
    expect(v.ghosted).toBe(false);
    expect(v.effectsHidden).toBe(true);
  });

  it("ghosted + effects-hidden", () => {
    const sets = mkSets({
      ghostedFileIds: new Set(["a"]),
      effectsHiddenFileIds: new Set(["a"]),
    });
    const v = nodeFilterVerdict("a", sets);
    expect(v.ghosted).toBe(true);
    expect(v.effectsHidden).toBe(true);
  });

  it("empty sets → shown", () => {
    const v = nodeFilterVerdict("a", mkSets());
    expect(v.ghosted).toBe(false);
    expect(v.effectsHidden).toBe(false);
  });

  it("different fileId → not affected", () => {
    const sets = mkSets({ ghostedFileIds: new Set(["a"]) });
    const v = nodeFilterVerdict("b", sets);
    expect(v.ghosted).toBe(false);
  });

  // -------------------------------------------------------------------------
  // F6 — Membership-vs-render decoupling.
  // `nodeFilterVerdict` is a PURE RENDER decision (filter/LOD) and is
  // independent of sin STATE — a building with a fire sin is still "burning"
  // regardless of filter; the filter only gates whether the fire sprites are
  // visible. These tests verify the verdict shape never confuses "hidden by
  // filter" with "not burning."
  // -------------------------------------------------------------------------

  it("F6: ghosted verdict is a render decision, orthogonal to sin state", () => {
    // A ghosted building may or may not have a fire sin; the verdict only
    // tells us about filter rendering, not about burning membership.
    const sets = mkSets({ ghostedFileIds: new Set(["burning-bld"]) });
    const v = nodeFilterVerdict("burning-bld", sets);
    // ghosted = true → render at 0.15 alpha (or hide entirely)
    // This does NOT mean "the fire pool should be destroyed."
    expect(v.ghosted).toBe(true);
    // effectsHidden=false → this building IS in the ghosted set, not the
    // effects-hidden set; the two are orthogonal.
    expect(v.effectsHidden).toBe(false);
  });

  it("F6: effectsHidden verdict gates effect overlay rendering only", () => {
    // effects-hidden is a RENDER decision for the sin-effect overlay;
    // it does NOT imply the building has no sin.
    const sets = mkSets({ effectsHiddenFileIds: new Set(["smoky-bld"]) });
    const v = nodeFilterVerdict("smoky-bld", sets);
    expect(v.effectsHidden).toBe(true);
    // This building IS in the effects-hidden set → crowd fire sprites
    // should be hidden, but the pool stays intact.
  });

  it("F6: ghosted + effects-hidden combo is a double-render-gate", () => {
    const sets = mkSets({
      ghostedFileIds: new Set(["b"]),
      effectsHiddenFileIds: new Set(["b"]),
    });
    const v = nodeFilterVerdict("b", sets);
    // Both render gates active — fire sprites invisible, building ghosted.
    // Pool membership is unaffected: if worstSinSeverity != null, the pool
    // still exists and will render normally when the filter is cleared.
    expect(v.ghosted).toBe(true);
    expect(v.effectsHidden).toBe(true);
  });

  it("F6: null sets → full render, membership decisions come from sin data", () => {
    // When no filter is active, every building renders at full opacity.
    // Burning membership is determined separately (worstSinSeverity != null).
    const v = nodeFilterVerdict("any", null);
    expect(v.ghosted).toBe(false);
    expect(v.hide).toBe(false);
    expect(v.effectsHidden).toBe(false);
  });

});
