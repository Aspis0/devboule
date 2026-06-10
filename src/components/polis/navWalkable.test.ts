import { describe, it, expect } from "vitest";
import { Container } from "pixi.js";
import {
  makeWaterBlocker,
  pathIsWalkable,
  pathTouchesBlocked,
  roundTile,
} from "./navWalkable";
import { RoadGraph } from "./roadGraph";
import { TradeRouteLayer } from "./TradeRouteLayer";
import type { Road, TerrainData } from "../../types/city";

// FRONTEND walkability guard: a citizen/porter polyline must never put a figure on
// open sea or an un-bridged river. The backend guarantees routed road tiles are
// walkable, so the guard drops NOTHING in practice — these tests inject a
// deliberately unsafe (degenerate) polyline to prove the guard rejects it, and a
// safe one to prove normal edges are untouched.

function mkRoad(
  roadId: string,
  from: string,
  to: string,
  path: { x: number; y: number }[],
  weight = 1,
): Road {
  return { roadId, from, to, type: "import", style: "lastricata", weight, path };
}

// A terrain frame with one river column (gx=5) crossed by a single bridge at
// (5,1), plus an open-sea margin at gx>=10. Everything else is land.
function terrainWithRiverAndSea(): TerrainData {
  const water = [];
  for (let gy = 0; gy < 4; gy++) {
    water.push({ gx: 5, gy, deep: false }); // river column
    water.push({ gx: 10, gy, deep: true }); // sea margin
    water.push({ gx: 11, gy, deep: true });
  }
  return {
    seaX: 10,
    minY: 0,
    maxY: 4,
    rivers: [{ gxMin: 5, gxMax: 5 }],
    water,
    sand: [],
    bridges: [{ gx: 5, gy: 1 }], // the SOLE river crossing
  };
}

// FIX 4 — coord→tile rounding must match the BACKEND exactly. The layout spirals
// around the origin so coords can be NEGATIVE. Rust `f64::round` rounds half AWAY
// FROM ZERO (-2.5 → -3); JS `Math.round` rounds half toward +∞ (-2.5 → -2). At a
// negative half-tile waypoint the two would map to DIFFERENT tiles, so the frontend
// guard could pass a tile the backend marks water (citizen on water) or wrongly
// drop a safe edge. `roundTile` reproduces the Rust semantics so they agree.
describe("roundTile (round-half-away-from-zero, matches Rust f64::round)", () => {
  it("rounds half AWAY from zero on both signs (unlike Math.round on negatives)", () => {
    // Positive halves: same as Math.round.
    expect(roundTile(2.5)).toBe(3);
    expect(roundTile(0.5)).toBe(1);
    // Negative halves: AWAY from zero — the divergence point.
    expect(roundTile(-2.5)).toBe(-3);
    expect(roundTile(-0.5)).toBe(-1);
    // This is exactly where JS Math.round disagrees (it would give -2 / -0).
    expect(roundTile(-2.5)).not.toBe(Math.round(-2.5));
    expect(Math.round(-2.5)).toBe(-2);
    // Non-half values round normally on both signs.
    expect(roundTile(-2.4)).toBe(-2);
    expect(roundTile(-2.6)).toBe(-3);
    expect(roundTile(2.4)).toBe(2);
    expect(roundTile(3)).toBe(3);
    expect(roundTile(0)).toBe(0);
  });

  it("a negative half-coord maps to the SAME tile the backend would (-2.5 → -3)", () => {
    // The backend (`terrain::road_tiles` / `nav::road_path_tiles`) emits the tile
    // for x=-2.5 via Rust round → -3. The frontend must agree.
    const RUST_ROUND_OF_NEG_2_5 = -3; // Rust `(-2.5_f64).round()` == -3.0
    expect(roundTile(-2.5)).toBe(RUST_ROUND_OF_NEG_2_5);
  });

  it("the blocked-tile check agrees with the backend at a negative half-coord", () => {
    // A river column at gx=-3 (the tile the backend assigns to x=-2.5). A waypoint
    // at x=-2.5 must be detected as blocked (it maps to -3), which JS Math.round
    // (→ -2, unblocked land) would have MISSED — a citizen on water.
    const terrain: TerrainData = {
      seaX: 100, // sea far away; this test is about the river column
      minY: 0,
      maxY: 4,
      rivers: [{ gxMin: -3, gxMax: -3 }],
      water: [
        { gx: -3, gy: 0, deep: false },
        { gx: -3, gy: 1, deep: false },
        { gx: -3, gy: 2, deep: false },
      ],
      sand: [],
      bridges: [], // un-bridged → blocked
    };
    const blocked = makeWaterBlocker(terrain);
    // x=-2.5 maps to tile -3 (Rust round) → blocked water.
    expect(blocked(-2.5, 1)).toBe(true);
    // The neighbouring land tile -2 (where JS Math.round(-2.5) would land) is NOT
    // blocked — proving the two roundings would have disagreed.
    expect(blocked(-2, 1)).toBe(false);

    // And a densified polyline whose interior waypoint sits at the negative
    // half-coord is rejected by the guard (it would touch the river tile -3).
    const unsafe = [
      { x: -5, y: 1 },
      { x: -2.5, y: 1 }, // → tile -3 (river), blocked
      { x: 0, y: 1 },
    ];
    expect(pathTouchesBlocked(unsafe, blocked)).toBe(true);
    expect(pathIsWalkable(unsafe, blocked)).toBe(false);
  });
});

describe("makeWaterBlocker", () => {
  it("blocks sea + un-bridged river tiles, but never a bridge or land", () => {
    const blocked = makeWaterBlocker(terrainWithRiverAndSea());
    // Un-bridged river tiles → blocked.
    expect(blocked(5, 0)).toBe(true);
    expect(blocked(5, 2)).toBe(true);
    // The bridge tile → walkable (NOT blocked) — the only river crossing.
    expect(blocked(5, 1)).toBe(false);
    // Open sea → blocked.
    expect(blocked(10, 0)).toBe(true);
    expect(blocked(11, 3)).toBe(true);
    // Land → walkable.
    expect(blocked(0, 0)).toBe(false);
    expect(blocked(4, 1)).toBe(false);
    expect(blocked(9, 1)).toBe(false);
  });

  it("blocks nothing for undefined / empty terrain (no behaviour change)", () => {
    const none = makeWaterBlocker(undefined);
    expect(none(5, 0)).toBe(false);
    const empty = makeWaterBlocker({
      seaX: 0,
      minY: 0,
      maxY: 0,
      rivers: [],
      water: [],
      sand: [],
      bridges: [],
    });
    expect(empty(5, 0)).toBe(false);
  });

  it("rounds fractional waypoint coords to the tile before testing", () => {
    const blocked = makeWaterBlocker(terrainWithRiverAndSea());
    expect(blocked(5.4, 2.1)).toBe(true); // rounds to (5,2) — river
    expect(blocked(4.9, 0.1)).toBe(true); // rounds to (5,0) — river
  });
});

describe("pathIsWalkable", () => {
  it("accepts a land/bridge polyline and rejects one that touches water", () => {
    const blocked = makeWaterBlocker(terrainWithRiverAndSea());
    // Crosses the river only via the bridge tile (5,1) — walkable.
    const safe = [
      { x: 3, y: 1 },
      { x: 5, y: 1 },
      { x: 8, y: 1 },
    ];
    expect(pathIsWalkable(safe, blocked)).toBe(true);
    // A waypoint on an un-bridged river tile (5,3) — NOT walkable.
    const unsafe = [
      { x: 3, y: 3 },
      { x: 5, y: 3 },
      { x: 8, y: 3 },
    ];
    expect(pathIsWalkable(unsafe, blocked)).toBe(false);
  });

  // THE corner-only hole: a HORIZONTAL run between two DRY corners that crosses an
  // un-bridged river column at an INTERIOR tile. A corner-only check would wrongly
  // pass it; densifying catches the interior river tile (5,3).
  it("catches an INTERIOR un-bridged river tile between two dry corners", () => {
    const blocked = makeWaterBlocker(terrainWithRiverAndSea());
    const interiorCrossing = [
      { x: 3, y: 3 }, // dry land
      { x: 8, y: 3 }, // dry land — but the run passes through river (5,3)
    ];
    expect(pathIsWalkable(interiorCrossing, blocked)).toBe(false);
    expect(pathTouchesBlocked(interiorCrossing, blocked)).toBe(true);
    // Same run one row up, where (5,1) IS a bridge → walkable end-to-end.
    const overBridge = [
      { x: 3, y: 1 },
      { x: 8, y: 1 },
    ];
    expect(pathIsWalkable(overBridge, blocked)).toBe(true);
  });
});

describe("RoadGraph walkability guard", () => {
  const terrain = terrainWithRiverAndSea();
  const blocked = makeWaterBlocker(terrain);

  it("keeps a safe edge (crossing via the bridge) and routes over it", () => {
    const roads = [
      mkRoad("safe", "a", "b", [
        { x: 3, y: 1 },
        { x: 5, y: 1 }, // bridge tile — walkable
        { x: 8, y: 1 },
      ]),
    ];
    const g = new RoadGraph(roads, blocked);
    expect(g.has("a")).toBe(true);
    expect(g.has("b")).toBe(true);
    expect(g.findRoute("a", "b")).not.toBeNull();
  });

  it("REJECTS an edge whose polyline crosses an un-bridged river tile", () => {
    const roads = [
      mkRoad("unsafe", "a", "b", [
        { x: 3, y: 3 },
        { x: 5, y: 3 }, // un-bridged river — blocked
        { x: 8, y: 3 },
      ]),
    ];
    const g = new RoadGraph(roads, blocked);
    // The edge was dropped → neither endpoint is a graph node, no route exists.
    expect(g.has("a")).toBe(false);
    expect(g.has("b")).toBe(false);
    expect(g.findRoute("a", "b")).toBeNull();
  });

  it("REJECTS an edge whose polyline runs onto the open sea", () => {
    const roads = [
      mkRoad("intosea", "a", "b", [
        { x: 8, y: 1 },
        { x: 11, y: 1 }, // gx >= seaX → sea — blocked
      ]),
    ];
    const g = new RoadGraph(roads, blocked);
    expect(g.has("a")).toBe(false);
    expect(g.findRoute("a", "b")).toBeNull();
  });

  it("without a blocker, keeps every edge (no behaviour change for old callers)", () => {
    const roads = [
      mkRoad("unsafe", "a", "b", [
        { x: 3, y: 3 },
        { x: 5, y: 3 },
        { x: 8, y: 3 },
      ]),
    ];
    const g = new RoadGraph(roads); // no guard
    expect(g.has("a")).toBe(true);
    expect(g.findRoute("a", "b")).not.toBeNull();
  });
});

describe("TradeRouteLayer walkability guard", () => {
  const terrain = terrainWithRiverAndSea();
  const blocked = makeWaterBlocker(terrain);
  // All endpoints resolve to an on-map anchor (the layer only needs presence).
  const resolve = () => ({ x: 0, y: 0 });

  it("spawns porters on a safe (bridge-crossing) edge", () => {
    const layer = new TradeRouteLayer(new Container());
    const roads = [
      mkRoad("safe", "a", "b", [
        { x: 3, y: 1 },
        { x: 5, y: 1 },
        { x: 8, y: 1 },
      ], 5),
    ];
    layer.setWorld(roads, resolve, blocked);
    expect(layer.count).toBeGreaterThan(0);
    layer.clear();
  });

  it("spawns NO porter on an edge that touches un-bridged water", () => {
    const layer = new TradeRouteLayer(new Container());
    const roads = [
      mkRoad("unsafe", "a", "b", [
        { x: 3, y: 3 },
        { x: 5, y: 3 }, // un-bridged river
        { x: 8, y: 3 },
      ], 5),
    ];
    layer.setWorld(roads, resolve, blocked);
    expect(layer.count).toBe(0);
    layer.clear();
  });
});
