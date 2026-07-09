// navWalkable.ts — FRONTEND walkability guard (belt-and-suspenders for the
// "citizens walk only on roads/plaza/bridges, never on water/buildings" rule).
//
// ┌─────────────────────────────────────────────────────────────────────────┐
// │ WHY THIS EXISTS                                                          │
// │                                                                          │
// │ The backend guarantees this BY CONSTRUCTION: roads are routed AROUND     │
// │ building footprints and a road tile over a river is marked `Bridge`, so  │
// │ every routed `Road.path` tile is walkable (proven by the Rust            │
// │ `nav::road_paths_all_walkable` test). The frontend citizens/porters/     │
// │ agents only ever interpolate ALONG those routed polylines, so they are   │
// │ already on roads/bridges.                                                │
// │                                                                          │
// │ This module is a CHEAP DEFENSIVE GUARD: it lets a layer reject a road    │
// │ polyline that — through some future bug or a degenerate edge — would put │
// │ a figure on a non-walkable tile (open sea or an un-bridged river). It    │
// │ never INVENTS geometry; it only filters out an unsafe segment so a       │
// │ citizen can never end up standing on water.                              │
// └─────────────────────────────────────────────────────────────────────────┘
//
// A tile is NON-walkable iff it is a water tile (sea OR river) that is NOT a
// bridge. Roads / plaza / land / sand are all fine for a road polyline (the
// polyline itself is the road; we only need to keep it off WATER). Mirrors the
// Rust `nav::walkable` exclusion: the only water a citizen may stand on is a
// `Bridge`. Built ONCE per terrain frame from the sparse wire payload (water +
// bridge tile lists), then a polyline is checked in O(waypoints).

import type { Building, TerrainData } from "../../types/city";

/**
 * Round a cartesian coordinate to its tile index, matching the BACKEND exactly.
 *
 * The layout spirals around the origin, so building/road coords can be NEGATIVE.
 * The Rust backend emits tiles with `f64::round`, which rounds half AWAY FROM ZERO
 * (`-2.5 → -3`, `2.5 → 3`). JS `Math.round` rounds half toward +∞ (`-2.5 → -2`),
 * so on a negative half-tile waypoint the two would map to DIFFERENT tiles — the
 * frontend guard could then pass a tile the backend marks water (a citizen on
 * water) or wrongly reject a safe edge. This helper reproduces Rust's
 * round-half-away-from-zero so the frontend tile mapping agrees with the backend
 * (`terrain::building_tiles` / `terrain::road_tiles`, `nav::road_path_tiles`)
 * tile-for-tile, including at negative half-coordinates.
 */
export function roundTile(v: number): number {
  return v < 0 ? -Math.round(-v) : Math.round(v);
}

/** Pack a tile coordinate into a single number key for a Set lookup. Tiles are
 *  small signed ints (a few hundred each way); a 16-bit-offset pack is collision
 *  free for any realistic city extent. */
function tileKey(gx: number, gy: number): number {
  // Offset into non-negative range, then interleave into one integer. The range
  // (±32767) dwarfs any real Polis extent (a few hundred tiles).
  return ((gx + 0x8000) << 16) | (gy + 0x8000);
}

/**
 * A precomputed predicate over the terrain frame: `blocked(gx, gy)` is true for a
 * tile a citizen must NEVER stand on — open sea or an un-bridged river. Bridges
 * (water with a road deck) are walkable, so they are NOT blocked.
 *
 * Returns a function that is cheap to call per waypoint. When `terrain` is absent
 * (a pre-terrain city) nothing is blocked (there is no water frame at all).
 */
export function makeWaterBlocker(
  terrain: TerrainData | undefined,
): (gx: number, gy: number) => boolean {
  if (!terrain || terrain.water.length === 0) {
    return () => false;
  }
  // Bridge tiles are walkable water — exclude them from the blocked set.
  const bridges = new Set<number>();
  for (const b of terrain.bridges) bridges.add(tileKey(b.gx, b.gy));

  const blocked = new Set<number>();
  for (const w of terrain.water) {
    const k = tileKey(w.gx, w.gy);
    if (!bridges.has(k)) blocked.add(k);
  }
  return (gx, gy) => blocked.has(tileKey(roundTile(gx), roundTile(gy)));
}

/**
 * Extract building FOOTPRINT tiles (one tile per building coordinate, NO
 * neighbourhood padding). Walkers may hug walls; the 4-neighbourhood expansion
 * that props need stays local in `occupiedTiles`.
 *
 * Exported so `makeBuildingBlocker` and `props.ts` share one source of truth.
 */
export function buildingFootprintTiles(
  coords: { x: number; y: number }[],
): Set<number> {
  const set = new Set<number>();
  for (const c of coords) {
    set.add(tileKey(roundTile(c.x), roundTile(c.y)));
  }
  return set;
}

/**
 * Precomputed predicate: `blocked(gx, gy)` is true for a building FOOTPRINT
 * tile only (no neighbourhood padding — walkers may hug walls). The caller
 * passes `buildings[].coords` from the city data.
 *
 * Built ONCE per city load from the building list. The closure captures a
 * prebuilt `Set<number>` of tile keys, so calls are O(1) Set lookups.
 */
export function makeBuildingBlocker(
  buildings: readonly Building[] | undefined,
): (gx: number, gy: number) => boolean {
  if (!buildings || buildings.length === 0) return () => false;
  const coords = buildings.filter((b) => b.coords).map((b) => b.coords!);
  if (coords.length === 0) return () => false;
  const footprints = buildingFootprintTiles(coords);
  return (gx, gy) => footprints.has(tileKey(roundTile(gx), roundTile(gy)));
}

/**
 * OR-composition of multiple blocked predicates. Returns a single predicate
 * that is true when ANY of the source blockers is true.
 *
 * Built ONCE per city load (typically water + building blockers). The returned
 * closure captures the array, so a call evaluates each blocker in order and
 * short-circuits on the first `true`.
 */
export function combineBlockers(
  ...blockers: ((gx: number, gy: number) => boolean)[]
): (gx: number, gy: number) => boolean {
  if (blockers.length === 0) return () => false;
  if (blockers.length === 1) return blockers[0];
  return (gx, gy) => {
    for (const b of blockers) {
      if (b(gx, gy)) return true;
    }
    return false;
  };
}

/**
 * Is every tile a CARTESIAN road polyline passes through walkable (i.e. none lands
 * on a blocked water tile)? Used to reject an unsafe edge before a citizen walks it.
 *
 * We DENSIFY each segment — stepping tile-by-tile along the changing axis — not
 * just the corner waypoints. This is the load-bearing correctness point of the
 * guard: a river is a 1-wide VERTICAL column, so a HORIZONTAL run between two LAND
 * corners can cross an un-bridged river tile at an INTERIOR position (both corners
 * dry, an inner tile water). A corner-only check would miss exactly that — the very
 * degenerate the guard exists to catch. Densifying mirrors the backend's road
 * rasterization (`terrain::road_tiles` / `nav::road_path_tiles`) tile-for-tile, so
 * the guard's verdict matches the bridge marking. O(tiles-on-path), still cheap
 * (a few-hundred-tile polyline), and only run at graph-build / setWorld time.
 */
export function pathIsWalkable(
  path: readonly { x: number; y: number }[],
  blocked: (gx: number, gy: number) => boolean,
): boolean {
  return !pathTouchesBlocked(path, blocked);
}

/** True if any tile a cartesian road polyline passes through is blocked (open sea
 *  / un-bridged river). Densifies each axis-aligned segment so an INTERIOR tile of
 *  a run (e.g. a horizontal run crossing a 1-wide un-bridged river column) is
 *  caught, not only the corner waypoints. Mirrors the backend road rasterization. */
export function pathTouchesBlocked(
  path: readonly { x: number; y: number }[],
  blocked: (gx: number, gy: number) => boolean,
): boolean {
  if (path.length === 0) return false;
  let px = roundTile(path[0].x);
  let py = roundTile(path[0].y);
  if (blocked(px, py)) return true;
  for (let i = 1; i < path.length; i++) {
    const cx = roundTile(path[i].x);
    const cy = roundTile(path[i].y);
    const dx = Math.sign(cx - px);
    const dy = Math.sign(cy - py);
    // Step toward the corner; for a clean axis-aligned run only one axis moves.
    // A defensive diagonal (shouldn't occur) still terminates (both axes converge).
    while (px !== cx || py !== cy) {
      if (px !== cx) px += dx;
      if (py !== cy) py += dy;
      if (blocked(px, py)) return true;
    }
  }
  return false;
}
