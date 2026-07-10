// resources.test.ts — unit tests for the quarry/mine resource site planner.
//
// Covers: threshold, kind/variant selection, overlap avoidance, determinism,
// and that sites are placed outside district bbox.

import { describe, expect, it } from "vitest";
import { planResourceSites, resourceSiteTiles, type ResourceCity } from "./resources";
import type { CityState } from "../../types/city";

/** Minimal CityState fixture for testing. */
function makeCity(overrides: Partial<CityState> = {}): CityState {
  return {
    version: 1,
    projectName: "test",
    era: "test",
    generatedAt: "",
    gridSize: { w: 100, h: 100 },
    districts: [],
    buildings: [],
    roads: [],
    agents: [],
    externalServices: [],
    notes: [],
    sins: [],
    ...overrides,
  };
}

describe("planResourceSites", () => {
  const lowCensusDistrict = {
    districtId: "d1",
    name: "Small Quarter",
    type: "feature" as const,
    bounds: { x: 10, y: 10, w: 6, h: 6 },
    wallStyle: "none" as const,
    colorAccent: "#888888",
    assetCensus: { images: 3, fonts: 2, media: 2 }, // total 7 — below threshold
  };

  const highCensusQuarryDistrict = {
    districtId: "d2",
    name: "Image Quarter",
    type: "feature" as const,
    bounds: { x: 30, y: 30, w: 8, h: 6 },
    wallStyle: "none" as const,
    colorAccent: "#888888",
    assetCensus: { images: 20, fonts: 3, media: 2 }, // total 25, images > fonts+media → quarry
  };

  const highCensusMineDistrict = {
    districtId: "d3",
    name: "Media Quarter",
    type: "feature" as const,
    bounds: { x: 60, y: 60, w: 8, h: 8 },
    wallStyle: "none" as const,
    colorAccent: "#888888",
    assetCensus: { images: 3, fonts: 8, media: 8 }, // total 19, images <= fonts+media → mine
  };

  it("returns empty when no district meets the threshold", () => {
    const city = makeCity({ districts: [lowCensusDistrict] });
    expect(planResourceSites(city)).toEqual([]);
  });

  it("returns empty when no districts exist", () => {
    const city = makeCity({ districts: [] });
    expect(planResourceSites(city)).toEqual([]);
  });

  it("places a quarry when images > fonts+media", () => {
    const city = makeCity({ districts: [highCensusQuarryDistrict] });
    const sites = planResourceSites(city);
    expect(sites).toHaveLength(1);
    expect(sites[0].kind).toBe("quarry");
    expect(sites[0].districtId).toBe("d2");
    expect(sites[0].districtLabel).toBe("Image Quarter");
    expect(sites[0].census).toEqual({ images: 20, fonts: 3, media: 2 });
  });

  it("places a mine when images <= fonts+media", () => {
    const city = makeCity({ districts: [highCensusMineDistrict] });
    const sites = planResourceSites(city);
    expect(sites).toHaveLength(1);
    expect(sites[0].kind).toBe("mine");
    expect(sites[0].districtId).toBe("d3");
  });

  it("quarry variant is deterministic (v0 or v1)", () => {
    const city = makeCity({ districts: [highCensusQuarryDistrict] });
    const sites = planResourceSites(city);
    expect(sites[0].variant).toBeGreaterThanOrEqual(0);
    expect(sites[0].variant).toBeLessThanOrEqual(1);
  });

  it("mine variant is always 0", () => {
    const city = makeCity({ districts: [highCensusMineDistrict] });
    const sites = planResourceSites(city);
    expect(sites[0].variant).toBe(0);
  });

  it("places sites outside district bbox", () => {
    const city = makeCity({ districts: [highCensusQuarryDistrict] });
    const sites = planResourceSites(city);
    const d = highCensusQuarryDistrict.bounds;
    const s = sites[0];
    const siteRight = s.gx + 3; // quarry is 3 wide
    const siteBottom = s.gy + 3; // quarry is 3 tall
    // The site rect must not overlap the district rect.
    const overlaps =
      s.gx < d.x + d.w && siteRight > d.x && s.gy < d.y + d.h && siteBottom > d.y;
    expect(overlaps).toBe(false);
  });

  it("is deterministic: same input → same output", () => {
    const city = makeCity({
      districts: [lowCensusDistrict, highCensusQuarryDistrict, highCensusMineDistrict],
    });
    const a = planResourceSites(city);
    const b = planResourceSites(city);
    expect(a).toEqual(b);
  });

  it("places one site per qualifying district", () => {
    const city = makeCity({
      districts: [lowCensusDistrict, highCensusQuarryDistrict, highCensusMineDistrict],
    });
    const sites = planResourceSites(city);
    // d1 (total 7) below threshold → no site
    // d2 (total 25) → quarry
    // d3 (total 19) → mine
    expect(sites).toHaveLength(2);
    const ids = sites.map((s) => s.districtId).sort();
    expect(ids).toEqual(["d2", "d3"]);
  });

  it("avoids building footprints", () => {
    const city = makeCity({
      districts: [highCensusQuarryDistrict],
      buildings: [
        {
          fileId: "f1",
          filePath: "f1.ts",
          districtId: "d2",
          purpose: "house",
          purposeSource: "heuristic",
          linesOfCode: 100,
          visualTier: "kalybe",
          coords: { x: 30 + 8 + 4, y: 33 }, // right where the E midpoint would land
          status: "normal",
          label: "f1",
          description: "",
          lastModified: "",
          sins: [],
          notes: [],
        },
      ],
    });
    const sites = planResourceSites(city);
    expect(sites).toHaveLength(1);
    // The site should NOT be at the building location.
    expect(sites[0].gx).not.toBe(30 + 8 + 4);
  });

  it("avoids water tiles", () => {
    const city = makeCity({
      districts: [highCensusQuarryDistrict],
      terrain: {
        seaX: 100,
        minY: 0,
        maxY: 100,
        rivers: [],
        water: [{ gx: 38, gy: 33, deep: false }], // right where E midpoint might land
        sand: [],
        bridges: [],
      },
    });
    const sites = planResourceSites(city);
    expect(sites).toHaveLength(1);
    // Site should not overlap the water tile.
    const s = sites[0];
    expect(`${s.gx},${s.gy}`).not.toBe("38,33");
    expect(`${s.gx + 1},${s.gy + 1}`).not.toBe("38,33");
  });

  it("rejects candidates beyond seaX (coastline guard)", () => {
    // District near the east edge; seaX at 38 means the E midpoint would land
    // on or past the sea. The planner must skip it and try other directions.
    const nearSeaDistrict = {
      ...highCensusQuarryDistrict,
      districtId: "d5",
      bounds: { x: 28, y: 30, w: 8, h: 6 },
    };
    const city = makeCity({
      districts: [nearSeaDistrict],
      terrain: {
        seaX: 38,
        minY: 0,
        maxY: 100,
        rivers: [],
        water: [],
        sand: [],
        bridges: [],
      },
    });
    const sites = planResourceSites(city);
    if (sites.length > 0) {
      // If a site was placed, it must not extend past seaX.
      expect(sites[0].gx + 3).toBeLessThanOrEqual(38);
    }
    // Even if no site fits (all 4 directions hit sea/blocked), it should not crash.
  });

  it("accepts a partial object without gridSize (no crash)", () => {
    // T1 regression: the planner must accept ResourceCity which has no gridSize.
    const city: ResourceCity = {
      districts: [highCensusQuarryDistrict],
      buildings: [],
      roads: [],
    };
    const sites = planResourceSites(city);
    expect(sites).toHaveLength(1);
    expect(sites[0].kind).toBe("quarry");
  });

  it("plans sites in negative coordinate space", () => {
    // Polis world coords go negative (spiral around origin).
    const negDistrict = {
      ...highCensusQuarryDistrict,
      districtId: "d-neg",
      bounds: { x: -50, y: -40, w: 6, h: 6 },
    };
    const city: ResourceCity = {
      districts: [negDistrict],
      buildings: [],
      roads: [],
    };
    const sites = planResourceSites(city);
    expect(sites).toHaveLength(1);
    // Site must be near the district, not clamped to 0.
    expect(sites[0].gx).toBeLessThan(0);
  });

  it("rejects candidates outside terrain y-band", () => {
    const nearEdgeDistrict = {
      ...highCensusQuarryDistrict,
      districtId: "d-edge",
      bounds: { x: 30, y: 2, w: 6, h: 6 },
    };
    // N midpoint would place site at y = 2 - 3 - 4 = -5, below minY=0.
    const city: ResourceCity = {
      districts: [nearEdgeDistrict],
      buildings: [],
      roads: [],
      terrain: { seaX: 100, minY: 0, maxY: 100, rivers: [], water: [], sand: [], bridges: [] },
    };
    const sites = planResourceSites(city);
    for (const s of sites) {
      expect(s.gy).toBeGreaterThanOrEqual(0);
    }
  });

  it("skips census absent (sparse wire) gracefully", () => {
    const districtNoCensus = {
      ...highCensusQuarryDistrict,
      districtId: "d4",
      assetCensus: undefined,
    };
    const city = makeCity({ districts: [districtNoCensus] });
    const sites = planResourceSites(city);
    // total 0 < 8 → no site
    expect(sites).toEqual([]);
  });
});

describe("resourceSiteTiles", () => {
  it("returns the union of footprint tiles", () => {
    const sites = [
      {
        id: "a",
        districtId: "d1",
        districtLabel: "A",
        kind: "quarry" as const,
        variant: 0,
        gx: 5,
        gy: 5,
        census: { images: 10, fonts: 0, media: 0 },
      },
      {
        id: "b",
        districtId: "d2",
        districtLabel: "B",
        kind: "mine" as const,
        variant: 0,
        gx: 20,
        gy: 20,
        census: { images: 0, fonts: 10, media: 0 },
      },
    ];
    const tiles = resourceSiteTiles(sites);
    // Quarry: 3x3 = 9 tiles at (5,5) to (7,7)
    expect(tiles.has("5,5")).toBe(true);
    expect(tiles.has("7,7")).toBe(true);
    // Mine: 5x5 = 25 tiles at (20,20) to (24,24)
    expect(tiles.has("20,20")).toBe(true);
    expect(tiles.has("24,24")).toBe(true);
    // Total: 9 + 25 = 34
    expect(tiles.size).toBe(34);
  });
});
