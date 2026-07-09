// Fields — deterministic farmland parcel planner tests.

import { describe, it, expect } from "vitest";
import type { Bounds, TerrainData } from "../../types/city";
import type { TerrainExtent } from "./terrain";
import {
  planFields,
  parcelTiles,
  buildFieldBlockedSet,
  type FieldParcel,
} from "./fields";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function mkExtent(
  minX: number,
  minY: number,
  maxX: number,
  maxY: number,
): TerrainExtent {
  return { minX, minY, maxX, maxY };
}

function mkBounds(x: number, y: number, w: number, h: number): Bounds {
  return { x, y, w, h };
}

// A default large empty extent for testing.
const BIG_EXTENT = mkExtent(0, 0, 60, 60);
const CENTRE = { x: 30, y: 30 };

// ---------------------------------------------------------------------------
// 1. Determinism
// ---------------------------------------------------------------------------

describe("planFields determinism", () => {
  it("two calls with the same input give deeply equal results", () => {
    const input = {
      ext: BIG_EXTENT,
      districts: [],
      blocked: new Set<string>(),
      centre: CENTRE,
    };
    const a = planFields(input);
    const b = planFields(input);
    expect(a).toEqual(b);
  });

  it("deterministic with districts present", () => {
    const districts = [mkBounds(10, 10, 8, 8)];
    const input = {
      ext: BIG_EXTENT,
      districts,
      blocked: new Set<string>(),
      centre: CENTRE,
    };
    const a = planFields(input);
    const b = planFields(input);
    expect(a).toEqual(b);
  });

  it("deterministic with blocked tiles", () => {
    const blocked = new Set<string>();
    blocked.add("5,5");
    blocked.add("6,5");
    blocked.add("5,6");
    const input = {
      ext: BIG_EXTENT,
      districts: [],
      blocked,
      centre: CENTRE,
    };
    const a = planFields(input);
    const b = planFields(input);
    expect(a).toEqual(b);
  });
});

// ---------------------------------------------------------------------------
// 2. No parcel tile is blocked / in district / outside extent
// ---------------------------------------------------------------------------

describe("planFields validity", () => {
  it("no parcel tile is in blocked set", () => {
    const blocked = new Set<string>();
    // Add some scattered blocked tiles.
    for (let i = 5; i < 15; i++) blocked.add(`${i},10`);
    const parcels = planFields({
      ext: BIG_EXTENT,
      districts: [],
      blocked,
      centre: CENTRE,
    });
    for (const p of parcels) {
      for (let dy = 0; dy < p.h; dy++) {
        for (let dx = 0; dx < p.w; dx++) {
          expect(blocked.has(`${p.x + dx},${p.y + dy}`)).toBe(false);
        }
      }
    }
  });

  it("no parcel intersects any dilated district bounds", () => {
    const districts = [mkBounds(20, 20, 10, 10)];
    const parcels = planFields({
      ext: BIG_EXTENT,
      districts,
      blocked: new Set<string>(),
      centre: CENTRE,
    });
    // Dilated district: x=19..30, y=19..30
    for (const p of parcels) {
      for (let dy = 0; dy < p.h; dy++) {
        for (let dx = 0; dx < p.w; dx++) {
          const tx = p.x + dx;
          const ty = p.y + dy;
          // Should not be inside dilated bounds (19..30 inclusive).
          const insideDilated =
            tx >= 19 && tx <= 30 && ty >= 19 && ty <= 30;
          expect(insideDilated).toBe(false);
        }
      }
    }
  });

  it("all parcels are within the extent", () => {
    const ext = mkExtent(5, 5, 50, 50);
    const parcels = planFields({
      ext,
      districts: [],
      blocked: new Set<string>(),
      centre: { x: 27, y: 27 },
    });
    for (const p of parcels) {
      expect(p.x).toBeGreaterThanOrEqual(ext.minX);
      expect(p.y).toBeGreaterThanOrEqual(ext.minY);
      expect(p.x + p.w).toBeLessThanOrEqual(ext.maxX + 1);
      expect(p.y + p.h).toBeLessThanOrEqual(ext.maxY + 1);
    }
  });
});

// ---------------------------------------------------------------------------
// 3. Parcel sizes from allowed list; no overlap; ≥1 tile gap
// ---------------------------------------------------------------------------

const ALLOWED_SIZES: [number, number][] = [
  [8, 6],
  [7, 5],
  [6, 4],
  [5, 4],
  [4, 3],
];

describe("planFields parcel constraints", () => {
  it("all parcel sizes are from the allowed list", () => {
    const parcels = planFields({
      ext: BIG_EXTENT,
      districts: [],
      blocked: new Set<string>(),
      centre: CENTRE,
    });
    for (const p of parcels) {
      const found = ALLOWED_SIZES.some(([w, h]) => p.w === w && p.h === h);
      expect(found).toBe(true);
    }
  });

  it("parcels do not overlap and keep >= 1 tile gap", () => {
    const parcels = planFields({
      ext: BIG_EXTENT,
      districts: [],
      blocked: new Set<string>(),
      centre: CENTRE,
    });
    // For each parcel, build its INTERIOR + 1-tile ring using independent
    // hardcoded loops from the spec (not any planFields helper).  This must
    // fail if two parcels are edge-adjacent with zero gap.
    const allRings: Set<string>[] = [];
    for (const p of parcels) {
      const ring = new Set<string>();
      // Interior: p.x .. p.x+w-1  ×  p.y .. p.y+h-1
      for (let ty = p.y; ty < p.y + p.h; ty++) {
        for (let tx = p.x; tx < p.x + p.w; tx++) {
          ring.add(`${tx},${ty}`);
        }
      }
      // 1-tile border ring around interior (spec: Chebyshev +1).
      for (let ty = p.y - 1; ty <= p.y + p.h; ty++) {
        for (let tx = p.x - 1; tx <= p.x + p.w; tx++) {
          ring.add(`${tx},${ty}`);
        }
      }
      allRings.push(ring);
    }
    // Every parcel's INTERIOR tile must not appear in any prior parcel's
    // interior+ring set.  Two parcels with zero gap would have parcel B's
    // interior touching parcel A's ring.
    for (let i = 0; i < parcels.length; i++) {
      const p = parcels[i];
      for (let ty = p.y; ty < p.y + p.h; ty++) {
        for (let tx = p.x; tx < p.x + p.w; tx++) {
          const key = `${tx},${ty}`;
          for (let j = 0; j < i; j++) {
            if (allRings[j].has(key)) {
              expect.fail(
                `Parcel ${i} interior tile ${key} falls inside parcel ${j}'s ring`,
              );
            }
          }
        }
      }
    }
  });
});

// ---------------------------------------------------------------------------
// 4. Kind banding
// ---------------------------------------------------------------------------

describe("planFields kind banding", () => {
  it("on a tiny extent (halfMax < 2) all parcels are garden", () => {
    // 6x5 extent  ⇒  halfW=3, halfH=2.5  ⇒  halfMax=3 — not tiny.
    // Use a truly tiny one: 4x3 ⇒ halfW=2, halfH=1.5 ⇒ halfMax=2 — exactly
    // the boundary.  Push below by 1 to force isTinyExtent.
    const ext = mkExtent(0, 0, 3, 2); // halfW=1.5, halfH=1, halfMax=1.5
    const centre = { x: 1.5, y: 1 };
    const parcels = planFields({
      ext,
      districts: [],
      blocked: new Set<string>(),
      centre,
    });
    expect(parcels.length).toBeGreaterThan(0);
    for (const p of parcels) {
      expect(p.kind).toBe("garden");
    }
  });

  it("a parcel near centre is 'garden', one at far corner is 'fallow' or 'orchard'", () => {
    // Use a large extent so both bands exist.
    const ext = mkExtent(0, 0, 100, 100);
    const centre = { x: 50, y: 50 };
    const parcels = planFields({
      ext,
      districts: [],
      blocked: new Set<string>(),
      centre,
    });
    expect(parcels.length).toBeGreaterThan(0);

    // Find a parcel near centre (Chebyshev dist < 0.35).
    const nearCentre = parcels.find((p) => {
      const cx = p.x + p.w / 2;
      const cy = p.y + p.h / 2;
      const dx = Math.abs(cx - centre.x) / 50;
      const dy = Math.abs(cy - centre.y) / 50;
      return Math.max(dx, dy) < 0.35;
    });
    // Find a parcel far from centre (Chebyshev dist > 0.8).
    const farCorner = parcels.find((p) => {
      const cx = p.x + p.w / 2;
      const cy = p.y + p.h / 2;
      const dx = Math.abs(cx - centre.x) / 50;
      const dy = Math.abs(cy - centre.y) / 50;
      return Math.max(dx, dy) > 0.8;
    });

    if (nearCentre) {
      expect(nearCentre.kind).toBe("garden");
    }
    if (farCorner) {
      expect(farCorner.kind === "fallow" || farCorner.kind === "orchard").toBe(true);
    }
    // At least one of each should exist in a large enough extent.
    expect(nearCentre).toBeDefined();
    expect(farCorner).toBeDefined();
  });
});

// ---------------------------------------------------------------------------
// 5. Accents
// ---------------------------------------------------------------------------

describe("planFields accents", () => {
  it("accents never appear on garden parcels", () => {
    const parcels = planFields({
      ext: BIG_EXTENT,
      districts: [],
      blocked: new Set<string>(),
      centre: CENTRE,
    });
    for (const p of parcels) {
      if (p.kind === "garden") {
        expect(p.accents.shed).toBeUndefined();
        expect(p.accents.haystack).toBeUndefined();
      }
    }
  });

  it("shed and haystack never share a tile", () => {
    const parcels = planFields({
      ext: BIG_EXTENT,
      districts: [],
      blocked: new Set<string>(),
      centre: CENTRE,
    });
    for (const p of parcels) {
      if (p.accents.shed && p.accents.haystack) {
        expect(
          p.accents.shed.x !== p.accents.haystack.x ||
            p.accents.shed.y !== p.accents.haystack.y,
        ).toBe(true);
      }
    }
  });
});

// ---------------------------------------------------------------------------
// 6. parcelTiles
// ---------------------------------------------------------------------------

describe("parcelTiles", () => {
  it("returns exactly the union of parcel rects", () => {
    const parcels: FieldParcel[] = [
      { x: 0, y: 0, w: 3, h: 2, kind: "crops", accents: {}, seed: 1 },
      { x: 10, y: 10, w: 2, h: 2, kind: "orchard", accents: {}, seed: 2 },
    ];
    const tiles = parcelTiles(parcels);
    // Parcel 1: (0,0),(1,0),(2,0),(0,1),(1,1),(2,1)
    expect(tiles.has("0,0")).toBe(true);
    expect(tiles.has("2,1")).toBe(true);
    // Parcel 2: (10,10),(11,10),(10,11),(11,11)
    expect(tiles.has("10,10")).toBe(true);
    expect(tiles.has("11,11")).toBe(true);
    // No extras.
    expect(tiles.size).toBe(6 + 4);
  });
});

// ---------------------------------------------------------------------------
// 7. Cap
// ---------------------------------------------------------------------------

describe("planFields cap", () => {
  it("result length <= MAX_PARCELS even with a huge empty extent", () => {
    const ext = mkExtent(0, 0, 500, 500);
    const parcels = planFields({
      ext,
      districts: [],
      blocked: new Set<string>(),
      centre: { x: 250, y: 250 },
    });
    expect(parcels.length).toBeLessThanOrEqual(160);
  });
});

// ---------------------------------------------------------------------------
// 9. buildFieldBlockedSet
// ---------------------------------------------------------------------------

describe("fractional district bounds", () => {
  it("no parcel tile falls inside the dilated integer coverage of a fractional district", () => {
    // Fractional district bounds: centre at (4.5, 4.5), size 3.2×2.7.
    // Dilated: x=3.5, y=3.5, w=5.2, h=4.7.
    // Integer coverage: floor(3.5)=3, floor(3.5)=3, ceil(3.5+5.2)=9, ceil(3.5+4.7)=9.
    // So tiles 3..8 × 3..8 should be blocked.
    const ext = mkExtent(0, 0, 30, 30);
    const centre = { x: 15, y: 15 };
    const districts = [mkBounds(4.5, 4.5, 3.2, 2.7)];
    const parcels = planFields({ ext, districts, blocked: new Set<string>(), centre });

    // Verify no parcel tile falls inside the integer coverage of the dilated rect.
    const dilatedX1 = Math.floor(4.5 - 1); // 3
    const dilatedY1 = Math.floor(4.5 - 1); // 3
    const dilatedX2 = Math.ceil(4.5 + 3.2 + 1); // 9
    const dilatedY2 = Math.ceil(4.5 + 2.7 + 1); // 9

    for (const p of parcels) {
      for (let dy = 0; dy < p.h; dy++) {
        for (let dx = 0; dx < p.w; dx++) {
          const tx = p.x + dx;
          const ty = p.y + dy;
          const inside =
            tx >= dilatedX1 && tx < dilatedX2 && ty >= dilatedY1 && ty < dilatedY2;
          if (inside) {
            expect.fail(
              `Parcel tile (${tx},${ty}) falls inside dilated integer coverage of fractional district`,
            );
          }
        }
      }
    }
  });
});

describe("buildFieldBlockedSet", () => {
  it("includes dilated building tiles", () => {
    const buildings = [{ coords: [{ x: 10, y: 10 }] }];
    const blocked = buildFieldBlockedSet(buildings, [], undefined);
    // Building at (10,10) → dilated by 1 → tiles 9..11 in both axes.
    expect(blocked.has("10,10")).toBe(true);
    expect(blocked.has("9,9")).toBe(true);
    expect(blocked.has("11,11")).toBe(true);
    expect(blocked.has("8,10")).toBe(false); // 2 away, not dilated.
  });

  it("includes road tiles", () => {
    const roads = [
      { path: [{ x: 5, y: 5 }, { x: 6, y: 5 }, { x: 7, y: 5 }] },
    ];
    const blocked = buildFieldBlockedSet([], roads, undefined);
    expect(blocked.has("5,5")).toBe(true);
    expect(blocked.has("6,5")).toBe(true);
    expect(blocked.has("7,5")).toBe(true);
    expect(blocked.has("8,5")).toBe(false);
  });

  it("includes water tiles", () => {
    const terrain: TerrainData = {
      seaX: 20,
      minY: 0,
      maxY: 10,
      rivers: [],
      water: [
        { gx: 20, gy: 0, deep: true },
        { gx: 20, gy: 1, deep: true },
      ],
      sand: [],
      bridges: [],
    };
    const blocked = buildFieldBlockedSet([], [], terrain);
    expect(blocked.has("20,0")).toBe(true);
    expect(blocked.has("20,1")).toBe(true);
    expect(blocked.has("20,2")).toBe(false);
  });

  it("includes bridge tiles", () => {
    const terrain: TerrainData = {
      seaX: 20,
      minY: 0,
      maxY: 10,
      rivers: [],
      water: [],
      sand: [],
      bridges: [{ gx: 5, gy: 3 }],
    };
    const blocked = buildFieldBlockedSet([], [], terrain);
    expect(blocked.has("5,3")).toBe(true);
  });

  it("a tile 2 away from a building is free", () => {
    const buildings = [{ coords: [{ x: 10, y: 10 }] }];
    const blocked = buildFieldBlockedSet(buildings, [], undefined);
    expect(blocked.has("8,10")).toBe(false);
    expect(blocked.has("12,10")).toBe(false);
    expect(blocked.has("10,8")).toBe(false);
    expect(blocked.has("10,12")).toBe(false);
  });
});
