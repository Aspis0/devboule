// coast.test.ts — pier + shoreline planners (pure, no PIXI draw assertions).
//
// Pins Phase 5 COAST contracts:
//   - pier planner: determinism, water-reach, cap, no-water skip
//   - shoreline scatter: water adjacency, density cap, determinism
//   - LOD constants split (fields extend further than walls)

import { describe, expect, it } from "vitest";
import type { TerrainData } from "../../types/city";
import {
  planPiers,
  planShorelineDecor,
  pierWaterReach,
  sandAdjacentToWater,
  waterTileKeySet,
  HARBOR_PURPOSES,
  MAX_PIERS,
  MAX_SHORE_PROPS,
  type PierBuilding,
} from "./coast";
import { LOD_FIELDS, LOD_WALLS } from "./lod";
import {
  planForestPatches,
  drawProps,
  basePropCap,
  MAX_PROPS_RICH,
  MAX_PROPS_LEAN,
  MAX_PROPS_MINIMAL,
} from "./props";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeTerrain(partial: Partial<TerrainData> = {}): TerrainData {
  return {
    seaX: 20,
    minY: 0,
    maxY: 30,
    rivers: [],
    water: [],
    sand: [],
    bridges: [],
    ...partial,
  };
}

function harbor(
  fileId: string,
  x: number,
  y: number,
  purpose: string = "harbor",
): PierBuilding {
  return { fileId, purpose, coords: { x, y } };
}

// ---------------------------------------------------------------------------
// LOD constants split
// ---------------------------------------------------------------------------

describe("LOD fields/walls split", () => {
  it("fields extend further out than walls (lower zoom threshold)", () => {
    expect(LOD_FIELDS).toBe(0.22);
    expect(LOD_WALLS).toBe(0.3);
    expect(LOD_FIELDS).toBeLessThan(LOD_WALLS);
  });
});

// ---------------------------------------------------------------------------
// Pier planner
// ---------------------------------------------------------------------------

describe("planPiers", () => {
  it("is deterministic: same input → same output", () => {
    const terrain = makeTerrain({
      water: [
        { gx: 12, gy: 5, deep: false },
        { gx: 13, gy: 5, deep: false },
        { gx: 14, gy: 5, deep: true },
      ],
    });
    const buildings = [
      harbor("a/harbor.ts", 10, 5),
      harbor("b/pharos.ts", 10, 8, "lighthouse"),
    ];
    expect(planPiers(buildings, terrain)).toEqual(planPiers(buildings, terrain));
  });

  it("only plans piers for harbor/lighthouse purposes", () => {
    const terrain = makeTerrain({
      water: [
        { gx: 12, gy: 5, deep: false },
        { gx: 12, gy: 6, deep: false },
      ],
    });
    const buildings = [
      harbor("h1", 10, 5, "harbor"),
      harbor("h2", 10, 6, "lighthouse"),
      harbor("h3", 10, 7, "house"),
      harbor("h4", 10, 8, "market"),
    ];
    const plans = planPiers(buildings, terrain);
    expect(plans.every((p) => HARBOR_PURPOSES.has(
      buildings.find((b) => b.fileId === p.fileId)!.purpose,
    ))).toBe(true);
    expect(plans.map((p) => p.fileId).sort()).toEqual(["h1", "h2"]);
  });

  it("skips entirely when terrain has no water", () => {
    const terrain = makeTerrain({ water: [] });
    const buildings = [harbor("h1", 10, 5)];
    expect(planPiers(buildings, terrain)).toEqual([]);
  });

  it("skips when terrain is undefined", () => {
    expect(planPiers([harbor("h1", 10, 5)], undefined)).toEqual([]);
  });

  it("skips buildings with no water within reach", () => {
    // Water is far west of the building; pier only reaches east.
    const terrain = makeTerrain({
      water: [{ gx: 0, gy: 5, deep: false }],
    });
    const buildings = [harbor("far", 10, 5)];
    expect(planPiers(buildings, terrain)).toEqual([]);
  });

  it("requires water within PIER_REACH_MAX tiles east", () => {
    const waterSet = waterTileKeySet(
      makeTerrain({ water: [{ gx: 16, gy: 5, deep: false }] }),
    );
    // bx=10 → max reach gx=15; water at 16 is out of range.
    expect(pierWaterReach(10, 5, waterSet)).toBeNull();
    // Water at gx=14 is within reach (d=4).
    const near = waterTileKeySet(
      makeTerrain({ water: [{ gx: 14, gy: 5, deep: false }] }),
    );
    expect(pierWaterReach(10, 5, near)).toBe(4);
  });

  it("caps at MAX_PIERS, preferring nearest to water", () => {
    // 10 harbors at increasing distance from the sea column at gx=20.
    const water = Array.from({ length: 20 }, (_, i) => ({
      gx: 20,
      gy: i,
      deep: false as const,
    }));
    const terrain = makeTerrain({ water });
    const buildings: PierBuilding[] = [];
    for (let i = 0; i < 10; i++) {
      // Closer harbors have higher bx (nearer the sea column).
      buildings.push(harbor(`h${i}`, 15 + (i % 3), i * 2));
    }
    const plans = planPiers(buildings, terrain);
    expect(plans.length).toBeLessThanOrEqual(MAX_PIERS);
    expect(plans.length).toBe(MAX_PIERS);
    // Sorted by reach ascending.
    const reaches = plans.map((p) => pierWaterReach(p.bx, p.by, waterTileKeySet(terrain))!);
    for (let i = 1; i < reaches.length; i++) {
      expect(reaches[i]).toBeGreaterThanOrEqual(reaches[i - 1]);
    }
  });

  it("respects an explicit maxPiers override of 0", () => {
    const terrain = makeTerrain({
      water: [{ gx: 12, gy: 5, deep: false }],
    });
    expect(planPiers([harbor("h1", 10, 5)], terrain, 0)).toEqual([]);
  });

  it("pier length is in [2,4] and starts one tile east of the building", () => {
    const terrain = makeTerrain({
      water: [
        { gx: 12, gy: 5, deep: false },
        { gx: 13, gy: 5, deep: false },
      ],
    });
    const plans = planPiers([harbor("h1", 10, 5)], terrain);
    expect(plans).toHaveLength(1);
    expect(plans[0].startGx).toBe(11);
    expect(plans[0].length).toBeGreaterThanOrEqual(2);
    expect(plans[0].length).toBeLessThanOrEqual(4);
    expect(plans[0].posts).toBeGreaterThanOrEqual(2);
    expect(plans[0].posts).toBeLessThanOrEqual(3);
  });
});

// ---------------------------------------------------------------------------
// Shoreline scatter
// ---------------------------------------------------------------------------

describe("planShorelineDecor", () => {
  it("is deterministic", () => {
    const terrain = makeTerrain({
      water: [
        { gx: 10, gy: 5, deep: false },
        { gx: 10, gy: 6, deep: false },
        { gx: 11, gy: 5, deep: false },
      ],
      sand: [
        { gx: 9, gy: 5 },
        { gx: 9, gy: 6 },
        { gx: 10, gy: 4 },
        { gx: 11, gy: 4 },
        { gx: 12, gy: 5 },
      ],
    });
    expect(planShorelineDecor(terrain)).toEqual(planShorelineDecor(terrain));
  });

  it("only places props on sand adjacent to water", () => {
    const terrain = makeTerrain({
      water: [{ gx: 10, gy: 5, deep: false }],
      sand: [
        { gx: 9, gy: 5 }, // W neighbour — adjacent
        { gx: 10, gy: 4 }, // N neighbour — adjacent
        { gx: 0, gy: 0 }, // far inland — NOT adjacent
        { gx: 50, gy: 50 }, // far — NOT adjacent
      ],
    });
    const waterSet = waterTileKeySet(terrain);
    const items = planShorelineDecor(terrain);
    for (const item of items) {
      expect(sandAdjacentToWater(item.gx, item.gy, waterSet)).toBe(true);
    }
    // Far sand never appears.
    expect(items.every((i) => !(i.gx === 0 && i.gy === 0))).toBe(true);
    expect(items.every((i) => !(i.gx === 50 && i.gy === 50))).toBe(true);
  });

  it("returns empty when no sand or no water", () => {
    expect(planShorelineDecor(makeTerrain({ water: [], sand: [{ gx: 1, gy: 1 }] }))).toEqual([]);
    expect(
      planShorelineDecor(
        makeTerrain({ water: [{ gx: 1, gy: 1, deep: false }], sand: [] }),
      ),
    ).toEqual([]);
    expect(planShorelineDecor(undefined)).toEqual([]);
  });

  it("respects the density cap", () => {
    // Long shoreline: many sand tiles next to a water column.
    const water = Array.from({ length: 200 }, (_, i) => ({
      gx: 20,
      gy: i,
      deep: false as const,
    }));
    const sand = Array.from({ length: 200 }, (_, i) => ({ gx: 19, gy: i }));
    const terrain = makeTerrain({ water, sand });
    const items = planShorelineDecor(terrain);
    // Bound is the exported hard cap — not a looser proxy like sand.length.
    expect(items.length).toBeLessThanOrEqual(MAX_SHORE_PROPS);
    // Explicit override.
    expect(planShorelineDecor(terrain, 5).length).toBeLessThanOrEqual(5);
  });

  it("sandAdjacentToWater checks 4-neighbour cardinals only", () => {
    const waterSet = new Set(["5,5"]);
    expect(sandAdjacentToWater(4, 5, waterSet)).toBe(true);
    expect(sandAdjacentToWater(6, 5, waterSet)).toBe(true);
    expect(sandAdjacentToWater(5, 4, waterSet)).toBe(true);
    expect(sandAdjacentToWater(5, 6, waterSet)).toBe(true);
    // Diagonal only — not adjacent by the cardinal rule.
    expect(sandAdjacentToWater(4, 4, waterSet)).toBe(false);
    expect(sandAdjacentToWater(5, 5, waterSet)).toBe(false); // the water tile itself
  });

  it("shore-decor tile keys block prop scatter (no double placement)", () => {
    // Mirrors the renderer contract: planShorelineDecor runs first, its tile
    // keys are unioned into occupied, then drawProps skips those tiles.
    const water = Array.from({ length: 80 }, (_, i) => ({
      gx: 10,
      gy: i,
      deep: false as const,
    }));
    const sand = Array.from({ length: 80 }, (_, i) => ({ gx: 9, gy: i }));
    const terrain = makeTerrain({ water, sand });
    const items = planShorelineDecor(terrain);
    expect(items.length).toBeGreaterThan(0);

    const shoreBlocked = new Set(items.map((i) => `${i.gx},${i.gy}`));

    // Each shore tile, alone and blocked, yields zero props.
    for (const item of items) {
      const key = `${item.gx},${item.gy}`;
      expect(shoreBlocked.has(key)).toBe(true);
      const { propCount } = drawProps(
        { minX: item.gx, maxX: item.gx, minY: item.gy, maxY: item.gy },
        new Set([key]),
      );
      expect(propCount).toBe(0);
    }

    // Contrast: with free tiles, at least one shore cell places a prop across
    // the set — so the block is real, not a vacuous "nothing ever places".
    let freePlaced = 0;
    for (const item of items) {
      const { propCount } = drawProps(
        { minX: item.gx, maxX: item.gx, minY: item.gy, maxY: item.gy },
        new Set(),
      );
      freePlaced += propCount;
    }
    expect(freePlaced).toBeGreaterThan(0);
  });
});

// ---------------------------------------------------------------------------
// Forest tuning + profile-aware cap
// ---------------------------------------------------------------------------

describe("forest tuning + basePropCap", () => {
  const EXT = { minX: 0, maxX: 80, minY: 0, maxY: 80 };

  it("basePropCap: rich 3400, lean 2800, minimal untouched 2800", () => {
    expect(basePropCap("rich")).toBe(MAX_PROPS_RICH);
    expect(basePropCap("lean")).toBe(MAX_PROPS_LEAN);
    expect(basePropCap("minimal")).toBe(MAX_PROPS_MINIMAL);
    expect(MAX_PROPS_RICH).toBe(3400);
    expect(MAX_PROPS_LEAN).toBe(2800);
    expect(MAX_PROPS_MINIMAL).toBe(2800);
  });

  it("planForestPatches returns 5–8 main patches (or fewer on tiny extent)", () => {
    const { patches } = planForestPatches(EXT, new Set());
    // Outer ring absent without districts — only main lattice patches.
    expect(patches.length).toBeGreaterThanOrEqual(0);
    expect(patches.length).toBeLessThanOrEqual(8);
  });

  it("cap uses rich base by default and raises with patch count", () => {
    const { patches, cap } = planForestPatches(EXT, new Set());
    expect(cap).toBe(MAX_PROPS_RICH + patches.length * 120);
  });

  it("lean clamp keeps the historical 2800 base", () => {
    const { patches, cap } = planForestPatches(EXT, new Set(), { tier: "lean" });
    expect(cap).toBe(MAX_PROPS_LEAN + patches.length * 120);
  });

  it("minimal tier is untouched (same floor as lean)", () => {
    const { patches, cap } = planForestPatches(EXT, new Set(), {
      tier: "minimal",
    });
    expect(cap).toBe(MAX_PROPS_MINIMAL + patches.length * 120);
  });

  it("outer ring patches sit outside district bounds", () => {
    const districts = [{ bounds: { x: 20, y: 20, w: 20, h: 20 } }];
    const { patches } = planForestPatches(EXT, new Set(), { districts });
    // At least the main patches; outer may add more.
    const outerish = patches.filter((p) => {
      const b = districts[0].bounds;
      return !(
        p.cx >= b.x &&
        p.cx < b.x + b.w &&
        p.cy >= b.y &&
        p.cy < b.y + b.h
      );
    });
    // Every outerish centre is outside the district (by construction of filter).
    for (const p of outerish) {
      const b = districts[0].bounds;
      const inside =
        p.cx >= b.x && p.cx < b.x + b.w && p.cy >= b.y && p.cy < b.y + b.h;
      expect(inside).toBe(false);
    }
  });

  it("is deterministic with districts + tier", () => {
    const districts = [{ bounds: { x: 10, y: 10, w: 15, h: 15 } }];
    const a = planForestPatches(EXT, new Set(), { tier: "rich", districts });
    const b = planForestPatches(EXT, new Set(), { tier: "rich", districts });
    expect(a).toEqual(b);
  });
});
