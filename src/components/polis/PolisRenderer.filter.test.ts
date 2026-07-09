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
});
