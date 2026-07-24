// densityFloor.test.ts — small-city density floor pure planners.
//
// Small city (<30 buildings): higher prop/forest factors, tighter extent margin.
// Large city: identity factors / default margin (unchanged).

import { describe, it, expect } from "vitest";
import {
  SMALL_CITY_THRESHOLD,
  DEFAULT_EXTENT_MARGIN,
  extentMarginForBuildingCount,
  smallCityDensityFactors,
} from "./densityFloor";
import { computeExtent } from "./terrain";
import { planForestPatches, basePropCap } from "./props";
import { desiredAmbientCount } from "./AmbientLayer";

describe("smallCityDensityFactors", () => {
  it("raises prop/forest factors for a tiny city (e.g. 3 buildings)", () => {
    const f = smallCityDensityFactors(3);
    expect(f.active).toBe(true);
    expect(f.propFactor).toBeGreaterThan(1);
    expect(f.forestFactor).toBeGreaterThan(1);
    // Tasteful boost — not extreme
    expect(f.propFactor).toBeLessThan(2);
    expect(f.forestFactor).toBeLessThan(2);
  });

  it("is stronger for fewer buildings", () => {
    const tiny = smallCityDensityFactors(3);
    const mid = smallCityDensityFactors(20);
    expect(tiny.propFactor).toBeGreaterThan(mid.propFactor);
    expect(tiny.forestFactor).toBeGreaterThan(mid.forestFactor);
  });

  it("returns identity (unchanged) for large cities", () => {
    const f = smallCityDensityFactors(30);
    expect(f.active).toBe(false);
    expect(f.propFactor).toBe(1);
    expect(f.forestFactor).toBe(1);

    const big = smallCityDensityFactors(200);
    expect(big.active).toBe(false);
    expect(big.propFactor).toBe(1);
    expect(big.forestFactor).toBe(1);
  });

  it("is deterministic", () => {
    expect(smallCityDensityFactors(5)).toEqual(smallCityDensityFactors(5));
  });
});

describe("extentMarginForBuildingCount", () => {
  it("tightens margin for tiny cities", () => {
    expect(extentMarginForBuildingCount(3)).toBe(2);
    expect(extentMarginForBuildingCount(7)).toBe(2);
    expect(extentMarginForBuildingCount(15)).toBe(3);
  });

  it("keeps default margin for large cities", () => {
    expect(extentMarginForBuildingCount(30)).toBe(DEFAULT_EXTENT_MARGIN);
    expect(extentMarginForBuildingCount(100)).toBe(DEFAULT_EXTENT_MARGIN);
  });

  it("computeExtent with small-city margin hugs buildings tighter", () => {
    const coords = [
      { x: 10, y: 10 },
      { x: 11, y: 10 },
      { x: 10, y: 11 },
    ];
    const tight = computeExtent(
      coords,
      8,
      8,
      extentMarginForBuildingCount(coords.length),
    );
    const wide = computeExtent(coords, 8, 8, DEFAULT_EXTENT_MARGIN);
    // Tighter: smaller span
    const tightW = tight.maxX - tight.minX;
    const wideW = wide.maxX - wide.minX;
    expect(tightW).toBeLessThan(wideW);
    expect(tight.minX).toBeGreaterThan(wide.minX);
    expect(tight.maxX).toBeLessThan(wide.maxX);
  });
});

describe("small-city forest density floor", () => {
  const EXT = { minX: 0, minY: 0, maxX: 40, maxY: 40 };

  it("planForestPatches yields more patches for small buildingCount", () => {
    const occupied = new Set<string>();
    const small = planForestPatches(EXT, occupied, {
      tier: "rich",
      buildingCount: 3,
    });
    const large = planForestPatches(EXT, occupied, {
      tier: "rich",
      buildingCount: 80,
    });
    // Small city forest factor > 1 → want more patches (when candidates allow)
    expect(small.patches.length).toBeGreaterThanOrEqual(large.patches.length);
    // Cap rises with patch count
    expect(small.cap).toBeGreaterThanOrEqual(basePropCap("rich"));
  });

  it("large city forest plan matches factor=1 (unchanged)", () => {
    const occupied = new Set<string>();
    const a = planForestPatches(EXT, occupied, {
      tier: "rich",
      buildingCount: 100,
    });
    const b = planForestPatches(EXT, occupied, {
      tier: "rich",
      forestFactor: 1,
    });
    expect(a.patches).toEqual(b.patches);
    expect(a.cap).toBe(b.cap);
  });
});

describe("ambient walker density floor", () => {
  it("floors tiny node counts to at least 6 walkers (profile cap permitting)", () => {
    // 3 nodes * 0.4 = 1 → floor 6
    expect(desiredAmbientCount(3)).toBe(6);
    // 10 nodes * 0.4 = 4 → floor 6
    expect(desiredAmbientCount(10)).toBe(6);
    // 20 nodes * 0.4 = 8 → above floor
    expect(desiredAmbientCount(20)).toBe(8);
  });

  it("respects profile cap below the floor (minimal max=6 still ok)", () => {
    expect(desiredAmbientCount(3, 6)).toBe(6);
    // Tight cap wins over floor
    expect(desiredAmbientCount(3, 4)).toBe(4);
  });

  it("large cities unchanged by the floor", () => {
    // 100 nodes * 0.4 = 40
    expect(desiredAmbientCount(100)).toBe(40);
  });
});

describe("SMALL_CITY_THRESHOLD", () => {
  it("is 30", () => {
    expect(SMALL_CITY_THRESHOLD).toBe(30);
  });
});
