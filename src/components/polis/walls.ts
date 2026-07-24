// District walls — pure planner + static drawer for Polis F3 wall styles.
//
// The backend already emits District.wallStyle ("roman_wall" | "aqueduct" |
// "palisade" | "none"); this module is the FIRST renderer code that reads it.
// Walls hug the BUILDINGS' actual extents (AABB + margin), not the district's
// reserved layout box (which includes GAP + DISTRICT_MARGIN empty meadow).
// Chunked warm sandstone 2.5D bands (Caesar III / Zeus sand-stucco). Readable
// at viewport scales 0.3–0.9 but subordinate to buildings (alpha ~0.8). Baked
// once into Graphics; redrawn only when districts rebuild.
//
// SIZE GATE (by building count in district, not reserved bounds):
//   < 6  → null (clean meadow)
//   6–9  → low field-stone kerb (variant "low")
//   >=10 → full wallStyle fortification (variant "full")
// EMPTINESS GATE: built footprint area / wall outline area must reach
// WALL_MIN_BUILT_RATIO — walls mark dense fabric, not sparse scatter.
// Corner towers: roman_wall only, and only when building count >= 14.
//
// DETERMINISM: same (district, roads, buildings, water) → identical segments.
// No Math.random(). Gate placement is derived from road crossings or a single
// city-center-facing fallback gate. Merlon/stake jitter uses a position hash.
// HONESTY: walls decorate real member buildings — never invent a district/road.

import { Graphics } from "pixi.js";
import type { Bounds, Building, District, Road, WallStyle } from "../../types/city";
import { CONTACT_SHADOW } from "./contactShadow";
import { cartToIso, dist, lerp, type IsoPoint } from "./iso";
import { DERIVED, tierRank } from "./palette";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/** Known drawable wall styles (excludes "none" / unknown). */
export type DrawableWallStyle = "roman_wall" | "aqueduct" | "palisade";

export interface WallSegment {
  /** Cart start (tile space). */
  ax: number;
  ay: number;
  /** Cart end (tile space). */
  bx: number;
  by: number;
  /** Edge index: 0=N (y=min), 1=E (x=max), 2=S (y=max), 3=W (x=min). */
  side: 0 | 1 | 2 | 3;
}

export interface WallTower {
  x: number;
  y: number;
  corner: 0 | 1 | 2 | 3;
}

export interface WallGate {
  x: number;
  y: number;
  side: 0 | 1 | 2 | 3;
}

/** Full fortification vs rural low boundary (building-count gated). */
export type WallPlanVariant = "full" | "low";

export interface WallPlan {
  districtId: string;
  style: DrawableWallStyle;
  /**
   * "full" — wallStyle fortification (>= 10 buildings).
   * "low"  — field-stone kerb / hedge (6–9 buildings).
   * Districts with < 6 buildings never plan a wall (null).
   */
  variant: WallPlanVariant;
  segments: WallSegment[];
  towers: WallTower[];
  gates: WallGate[];
}

// Building-count thresholds for wall presence (counted by districtId).
/** Below this: no wall at all (clean meadow). */
const WALL_MIN_LOW = 6;
/** At/above this: full wall per wallStyle; 6..full-1 is low boundary. */
const WALL_MIN_FULL = 10;
/** Corner towers only for roman_wall districts at/above this count. */
const WALL_MIN_TOWERS = 14;

/**
 * Minimum built-footprint / outline-area ratio. Below this the cluster is
 * mostly empty meadow and a wall would read as a fence around a field.
 *
 * Fixture (`polis-dev-city.json`, GAP=3 layout): eligible districts span
 * ~0.10–0.29. A wall is rare and meaningful — only genuine dense fabric.
 * 0.25 keeps the top densest (oracle cores + views); the next band
 * (0.22–0.25) and the long 0.10–0.20 tail stay unwalled. Soft district
 * tint + corner stelae carry place identity for everyone else.
 */
export const WALL_MIN_BUILT_RATIO = 0.25;

/** Tile margin around the member buildings' AABB (wall sits just outside fabric). */
export const WALL_OUTLINE_MARGIN = 1;

/**
 * Draw geometry in unscaled iso/screen px (viewport scale multiplies these).
 * Exported so tests can pin mass and compute on-screen size at known zooms.
 */
export const WALL_GEOMETRY = {
  /** Full fortification band thickness (top face width). */
  bandW: 7,
  /** Full fortification height (screen-up extrusion). */
  wallH: 14,
  /** Low rural kerb thickness. */
  lowBandW: 4,
  /** Low rural kerb height. */
  lowWallH: 6,
  /** Corner tower footprint and height. */
  towerSize: 12,
  towerH: 18,
  /** Gate jamb size. */
  jambW: 6,
  jambH: 12,
  /** Merlon block. */
  merlonW: 4,
  merlonH: 5,
} as const;

/** Optional water tile set ("gx,gy") + city centre for fallback gate. */
export interface WallPlanOptions {
  waterTiles?: Set<string>;
  /** City centre in cart tile space (mean of buildings, or grid centre). */
  cityCenter?: { x: number; y: number };
  /**
   * Pre-built buildings-by-fileId map. When drawing many districts, the caller
   * builds this once and reuses it so `findRoadGates` does not reallocate
   * O(B) maps per district.
   */
  buildingsById?: Map<string, Building>;
}

// Gate half-width in tiles (full gap = 2 * GATE_HALF).
const GATE_HALF = 0.55;
// Sub-sample step along each edge when testing water / splitting segments.
const EDGE_STEP = 0.5;

// ---------------------------------------------------------------------------
// Building footprints (tile W×D) — pure mirror of Rust `footprint.rs` / kit
// builders. Planner must not spin Pixi kit builders just to measure extents.
// ---------------------------------------------------------------------------

/**
 * Tile footprint `[W, D]` for a purpose + visual tier.
 * Kit anchors origin at coords; building occupies `[x, x+W) × [y, y+D)`.
 * Unknown purpose/tier → `[1, 1]` (same as kit `unknown`).
 */
export function buildingTileFootprint(
  purpose: string,
  visualTier: string,
): { w: number; d: number } {
  const l = Math.max(0, Math.min(4, tierRank(visualTier)));
  // Each row: levels 0..4 as [W, D], mirrored from kitcd/buildings.ts + Rust.
  const table: Record<string, readonly (readonly [number, number])[]> = {
    temple: [
      [2, 3],
      [2, 3],
      [3, 4],
      [3, 5],
      [4, 6],
    ],
    house: [
      [1, 1],
      [1, 1],
      [2, 2],
      [2, 2],
      [3, 3],
    ],
    fortress: [
      [2, 2],
      [2, 2],
      [3, 3],
      [3, 4],
      [4, 4],
    ],
    tower: [
      [1, 1],
      [1, 1],
      [1, 1],
      [2, 2],
      [2, 2],
    ],
    lighthouse: [
      [2, 2],
      [2, 2],
      [2, 2],
      [2, 2],
      [2, 2],
    ],
    market: [
      [2, 2],
      [2, 3],
      [3, 3],
      [3, 4],
      [4, 4],
    ],
    warehouse: [
      [2, 2],
      [2, 3],
      [3, 3],
      [4, 3],
      [4, 4],
    ],
    workshop: [
      [1, 1],
      [2, 2],
      [2, 2],
      [3, 2],
      [3, 3],
    ],
    conduit: [
      [1, 2],
      [1, 3],
      [1, 3],
      [1, 4],
      [1, 5],
    ],
    baths: [
      [2, 2],
      [2, 3],
      [3, 3],
      [3, 4],
      [4, 4],
    ],
    theater: [
      [3, 2],
      [3, 3],
      [4, 3],
      [4, 4],
      [5, 4],
    ],
    harbor: [
      [2, 2],
      [3, 2],
      [3, 3],
      [4, 3],
      [4, 4],
    ],
    library: [
      [2, 2],
      [3, 2],
      [3, 3],
      [4, 3],
      [4, 3],
    ],
    townhall: [
      [2, 2],
      [3, 3],
      [3, 3],
      [4, 4],
      [4, 5],
    ],
  };
  const row = table[purpose];
  const pair = row ? row[l] : ([1, 1] as const);
  return { w: pair[0], d: pair[1] };
}

/**
 * AABB of member buildings' footprints plus {@link WALL_OUTLINE_MARGIN}.
 * Returns null when the district has no members or the box is degenerate.
 */
export function builtOutlineBounds(
  districtId: string,
  buildings: Building[],
  margin: number = WALL_OUTLINE_MARGIN,
): Bounds | null {
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  let any = false;
  for (const bld of buildings) {
    if (bld.districtId !== districtId) continue;
    const { w, d } = buildingTileFootprint(bld.purpose, bld.visualTier);
    const x0 = bld.coords.x;
    const y0 = bld.coords.y;
    const x1 = x0 + w;
    const y1 = y0 + d;
    if (x0 < minX) minX = x0;
    if (y0 < minY) minY = y0;
    if (x1 > maxX) maxX = x1;
    if (y1 > maxY) maxY = y1;
    any = true;
  }
  if (!any) return null;
  const x = minX - margin;
  const y = minY - margin;
  const bw = maxX - minX + 2 * margin;
  const bh = maxY - minY + 2 * margin;
  if (!(Number.isFinite(bw) && Number.isFinite(bh)) || bw <= 0 || bh <= 0) {
    return null;
  }
  return { x, y, w: bw, h: bh };
}

/** Sum of member building footprint areas (tile²). */
export function builtFootprintArea(
  districtId: string,
  buildings: Building[],
): number {
  let area = 0;
  for (const bld of buildings) {
    if (bld.districtId !== districtId) continue;
    const { w, d } = buildingTileFootprint(bld.purpose, bld.visualTier);
    area += w * d;
  }
  return area;
}

/**
 * Built footprint area / outline area. 0 when outline is missing/empty.
 * Used by the emptiness gate and by measurement tests.
 */
export function builtToEnclosedRatio(
  districtId: string,
  buildings: Building[],
  outline: Bounds | null = builtOutlineBounds(districtId, buildings),
): number {
  if (!outline || outline.w <= 0 || outline.h <= 0) return 0;
  return builtFootprintArea(districtId, buildings) / (outline.w * outline.h);
}

// ---------------------------------------------------------------------------
// Soft-boundary corner stelae (unwalled districts)
// ---------------------------------------------------------------------------

/** Corner boundary stone — mass object, not a tick. */
export interface BoundaryMarker {
  x: number;
  y: number;
  corner: 0 | 1 | 2 | 3;
}

/**
 * Screen-px stele mass (viewport scale multiplies). Smaller than wall towers
 * but still a readable stone block at working zoom.
 */
export const STELE_GEOMETRY = {
  /** Footprint width of the stele block. */
  w: 5,
  /** Height (screen-up extrusion). */
  h: 9,
} as const;

/**
 * Built-outline corner markers for districts that have enough buildings to
 * read as a place but may lack a wall. Same count floor as low walls.
 * PURE + deterministic. Null when membership is below the floor or empty.
 */
export function planBoundaryMarkers(
  districtId: string,
  buildings: Building[],
): BoundaryMarker[] | null {
  const n = countBuildingsInDistrict(districtId, buildings);
  if (n < WALL_MIN_LOW) return null;
  const outline = builtOutlineBounds(districtId, buildings);
  if (!outline) return null;
  const corners = boundsCorners(outline);
  return corners.map((c, i) => ({
    x: c.x,
    y: c.y,
    corner: i as 0 | 1 | 2 | 3,
  }));
}

/**
 * Draw corner stelae as small stone blocks with plinth + cap (mass, not ticks).
 * Returns ops drawn. Uses wall-stone DERIVED colours.
 */
export function drawBoundaryMarkers(
  g: Graphics,
  markers: BoundaryMarker[],
): number {
  let ops = 0;
  const w = STELE_GEOMETRY.w;
  const h = STELE_GEOMETRY.h;
  const half = w / 2;
  for (const m of markers) {
    const c = cartToIso(m.x, m.y);
    // Body.
    g.rect(c.x - half, c.y - h + 1, w, h).fill({
      color: DERIVED.wallStone,
      alpha: WALL_ALPHA.body,
    });
    // SE shade face.
    g.rect(c.x, c.y - h + 1, half, h).fill({
      color: DERIVED.wallStoneDark,
      alpha: WALL_ALPHA.body * 0.9,
    });
    // Cap slab.
    g.rect(c.x - half - 0.5, c.y - h - 1.5, w + 1, 3).fill({
      color: DERIVED.wallStoneDark,
      alpha: WALL_ALPHA.body,
    });
    // Cap highlight.
    g.rect(c.x - half - 0.5, c.y - h - 1.5, w + 1, 1.2).fill({
      color: DERIVED.wallStoneLight,
      alpha: WALL_ALPHA.top,
    });
    // Contact shadow.
    g.ellipse(c.x, c.y + 1.2, w * 0.5, w * 0.2).fill({
      color: DERIVED.wallStoneDark,
      alpha: CONTACT_SHADOW.alpha,
    });
    ops += 5;
  }
  return ops;
}

// ---------------------------------------------------------------------------
// Style resolution
// ---------------------------------------------------------------------------

export function resolveWallStyle(style: WallStyle | string | undefined): DrawableWallStyle | null {
  if (style === "roman_wall" || style === "aqueduct" || style === "palisade") return style;
  return null;
}

// ---------------------------------------------------------------------------
// Geometry helpers
// ---------------------------------------------------------------------------

/** Four corners of a bounds rect in cart space, CW from NW: NW, NE, SE, SW. */
function boundsCorners(b: Bounds): { x: number; y: number }[] {
  const { x, y, w, h } = b;
  return [
    { x, y }, // 0 NW
    { x: x + w, y }, // 1 NE
    { x: x + w, y: y + h }, // 2 SE
    { x, y: y + h }, // 3 SW
  ];
}

/** Parametric point on edge `side` at t in [0, 1]. */
function edgePoint(b: Bounds, side: 0 | 1 | 2 | 3, t: number): { x: number; y: number } {
  const c = boundsCorners(b);
  const a = c[side];
  const bpt = c[(side + 1) % 4];
  return { x: a.x + (bpt.x - a.x) * t, y: a.y + (bpt.y - a.y) * t };
}

/** Edge length in tiles. */
function edgeLength(b: Bounds, side: 0 | 1 | 2 | 3): number {
  return side === 0 || side === 2 ? b.w : b.h;
}

/** Clamp t to [0, 1]. */
function clamp01(t: number): number {
  return t < 0 ? 0 : t > 1 ? 1 : t;
}

/**
 * Project a cart point onto an edge; returns t in [0,1] and squared distance
 * to the edge segment (in tile units). Used to detect road crossings.
 */
function projectOntoEdge(
  b: Bounds,
  side: 0 | 1 | 2 | 3,
  px: number,
  py: number,
): { t: number; d2: number } {
  const c = boundsCorners(b);
  const ax = c[side].x;
  const ay = c[side].y;
  const bx = c[(side + 1) % 4].x;
  const by = c[(side + 1) % 4].y;
  const abx = bx - ax;
  const aby = by - ay;
  const len2 = abx * abx + aby * aby;
  if (len2 < 1e-9) return { t: 0, d2: (px - ax) * (px - ax) + (py - ay) * (py - ay) };
  let t = ((px - ax) * abx + (py - ay) * aby) / len2;
  t = clamp01(t);
  const qx = ax + abx * t;
  const qy = ay + aby * t;
  const dx = px - qx;
  const dy = py - qy;
  return { t, d2: dx * dx + dy * dy };
}

/**
 * Segment-segment intersection in 2D. Returns t along edge AB in [0,1] if the
 * open segments cross (or touch endpoints within the edge), else null.
 */
function segIntersectT(
  ax: number,
  ay: number,
  bx: number,
  by: number,
  cx: number,
  cy: number,
  dx: number,
  dy: number,
): number | null {
  const rX = bx - ax;
  const rY = by - ay;
  const sX = dx - cx;
  const sY = dy - cy;
  const den = rX * sY - rY * sX;
  if (Math.abs(den) < 1e-12) return null; // parallel
  const qpx = cx - ax;
  const qpy = cy - ay;
  const t = (qpx * sY - qpy * sX) / den;
  const u = (qpx * rY - qpy * rX) / den;
  if (t < -1e-9 || t > 1 + 1e-9 || u < -1e-9 || u > 1 + 1e-9) return null;
  return clamp01(t);
}

/** True if cart point is strictly inside bounds (not on boundary). */
function insideBounds(b: Bounds, x: number, y: number): boolean {
  return x > b.x + 1e-6 && x < b.x + b.w - 1e-6 && y > b.y + 1e-6 && y < b.y + b.h - 1e-6;
}

// ---------------------------------------------------------------------------
// Gate detection
// ---------------------------------------------------------------------------

/**
 * Collect gate centres on the wall outline where inter-district roads cross.
 * Prefers path polylines; falls back to straight from→to via buildings.
 * Sorted deterministically (side, then t).
 *
 * Matches `drawRoads`: roads whose endpoints are missing from `byId` are
 * skipped entirely (stale refs must not punch phantom gates).
 */
function findRoadGates(
  wallBounds: Bounds,
  districtId: string,
  roads: Road[],
  byId: Map<string, Building>,
): WallGate[] {
  const b = wallBounds;
  if (!(Number.isFinite(b.w) && Number.isFinite(b.h)) || b.w <= 0 || b.h <= 0) {
    return [];
  }

  // Collect candidate (side, t) with a small snap so near-duplicates merge.
  const raw: { side: 0 | 1 | 2 | 3; t: number }[] = [];
  const CROSS_D2 = 0.55 * 0.55; // ~0.55 tiles off the edge counts as a crossing

  const considerPoint = (px: number, py: number): void => {
    for (let side = 0; side < 4; side++) {
      const s = side as 0 | 1 | 2 | 3;
      const { t, d2 } = projectOntoEdge(b, s, px, py);
      if (d2 <= CROSS_D2 && t > 0.02 && t < 0.98) {
        raw.push({ side: s, t });
      }
    }
  };

  const considerSeg = (
    ax: number,
    ay: number,
    bx: number,
    by: number,
  ): void => {
    const c = boundsCorners(b);
    for (let side = 0; side < 4; side++) {
      const s = side as 0 | 1 | 2 | 3;
      const a = c[s];
      const bp = c[(s + 1) % 4];
      const t = segIntersectT(a.x, a.y, bp.x, bp.y, ax, ay, bx, by);
      if (t !== null && t > 0.02 && t < 0.98) raw.push({ side: s, t });
    }
  };

  for (const road of roads) {
    const fromB = byId.get(road.from);
    const toB = byId.get(road.to);
    // Match drawRoads: missing endpoints → skip (no phantom gate).
    if (!fromB || !toB) continue;
    // Inter-district only: at least one endpoint outside this district.
    // Same-district roads don't punch gates.
    const fromOut = fromB.districtId !== districtId;
    const toOut = toB.districtId !== districtId;
    if (!fromOut && !toOut) continue;

    if (road.path && road.path.length >= 2) {
      for (let i = 0; i < road.path.length - 1; i++) {
        const p0 = road.path[i];
        const p1 = road.path[i + 1];
        considerSeg(p0.x, p0.y, p1.x, p1.y);
        // Also snap near-edge waypoints (routed paths often land on the rim).
        considerPoint(p0.x, p0.y);
      }
      const last = road.path[road.path.length - 1];
      considerPoint(last.x, last.y);
    } else {
      // Straight fallback: only if the segment straddles inside/outside.
      const aIn = insideBounds(b, fromB.coords.x, fromB.coords.y);
      const bIn = insideBounds(b, toB.coords.x, toB.coords.y);
      if (aIn !== bIn || fromOut || toOut) {
        considerSeg(fromB.coords.x, fromB.coords.y, toB.coords.x, toB.coords.y);
      }
    }
  }

  if (raw.length === 0) return [];

  // Sort + merge near-duplicates (same side, t within GATE_HALF / edgeLen).
  raw.sort((a, b) => (a.side !== b.side ? a.side - b.side : a.t - b.t));
  const merged: WallGate[] = [];
  let lastSide: 0 | 1 | 2 | 3 | null = null;
  let lastT = -1;
  for (const g of raw) {
    const el = edgeLength(b, g.side) || 1;
    const minDt = (GATE_HALF * 1.6) / el;
    if (lastSide === g.side && Math.abs(lastT - g.t) < minDt) continue;
    const p = edgePoint(b, g.side, g.t);
    merged.push({ x: p.x, y: p.y, side: g.side });
    lastSide = g.side;
    lastT = g.t;
  }
  return merged;
}

/**
 * Single fallback gate on the side facing the city centre (or east if centre
 * is missing / coincides with outline centre). Probes several t values so the
 * gate prefers dry land when the midpoint sits on water.
 */
function fallbackGate(
  wallBounds: Bounds,
  cityCenter: { x: number; y: number } | undefined,
  water: Set<string> | undefined,
): WallGate {
  const b = wallBounds;
  const cx = b.x + b.w / 2;
  const cy = b.y + b.h / 2;
  const tx = cityCenter?.x ?? cx + 1;
  const ty = cityCenter?.y ?? cy;
  const dx = tx - cx;
  const dy = ty - cy;
  // Pick the side the vector exits: dominate by abs component.
  let side: 0 | 1 | 2 | 3;
  if (Math.abs(dx) >= Math.abs(dy)) {
    side = dx >= 0 ? 1 : 3; // E or W
  } else {
    side = dy >= 0 ? 2 : 0; // S or N
  }
  // Probe order: midpoint first, then symmetric offsets; keep 0.5 if all wet.
  const probes = [0.5, 0.35, 0.65, 0.25, 0.75];
  for (const t of probes) {
    const p = edgePoint(b, side, t);
    if (!isWater(water, p.x, p.y)) {
      return { x: p.x, y: p.y, side };
    }
  }
  const mid = edgePoint(b, side, 0.5);
  return { x: mid.x, y: mid.y, side };
}

// ---------------------------------------------------------------------------
// Segment planning
// ---------------------------------------------------------------------------

/** True if the tile containing (x,y) is water. */
function isWater(water: Set<string> | undefined, x: number, y: number): boolean {
  if (!water || water.size === 0) return false;
  return water.has(`${Math.floor(x)},${Math.floor(y)}`);
}

/**
 * Build wall runs along one edge, punching gate gaps and skipping water tiles.
 * Returns cart-space segments on that side.
 */
function planEdgeSegments(
  b: Bounds,
  side: 0 | 1 | 2 | 3,
  gates: WallGate[],
  water: Set<string> | undefined,
): WallSegment[] {
  const el = edgeLength(b, side);
  if (el <= 0) return [];

  // Gate intervals in t-space on this side.
  const gaps: { t0: number; t1: number }[] = [];
  for (const g of gates) {
    if (g.side !== side) continue;
    const { t } = projectOntoEdge(b, side, g.x, g.y);
    const ht = GATE_HALF / el;
    gaps.push({ t0: clamp01(t - ht), t1: clamp01(t + ht) });
  }
  gaps.sort((a, b) => a.t0 - b.t0);
  // Merge overlapping gaps.
  const mergedGaps: { t0: number; t1: number }[] = [];
  for (const g of gaps) {
    const last = mergedGaps[mergedGaps.length - 1];
    if (last && g.t0 <= last.t1) last.t1 = Math.max(last.t1, g.t1);
    else mergedGaps.push({ ...g });
  }

  // Solid intervals = complement of gaps on [0,1].
  const solids: { t0: number; t1: number }[] = [];
  let cursor = 0;
  for (const g of mergedGaps) {
    if (g.t0 > cursor + 1e-6) solids.push({ t0: cursor, t1: g.t0 });
    cursor = Math.max(cursor, g.t1);
  }
  if (cursor < 1 - 1e-6) solids.push({ t0: cursor, t1: 1 });

  // Subdivide solids by water: walk at EDGE_STEP, emit contiguous non-water runs.
  const out: WallSegment[] = [];
  for (const s of solids) {
    const steps = Math.max(1, Math.ceil(((s.t1 - s.t0) * el) / EDGE_STEP));
    let runStart: number | null = null;
    for (let i = 0; i <= steps; i++) {
      const t = s.t0 + ((s.t1 - s.t0) * i) / steps;
      const p = edgePoint(b, side, t);
      const wet = isWater(water, p.x, p.y);
      if (!wet && runStart === null) runStart = t;
      if ((wet || i === steps) && runStart !== null) {
        const tEnd = wet ? t : s.t1;
        if (tEnd - runStart > 1e-4) {
          const a = edgePoint(b, side, runStart);
          const bp = edgePoint(b, side, tEnd);
          out.push({ ax: a.x, ay: a.y, bx: bp.x, by: bp.y, side });
        }
        runStart = null;
      }
    }
  }
  return out;
}

/**
 * Count buildings belonging to a district (by districtId). Used for wall size
 * thresholds — never inferred from bounds size.
 */
export function countBuildingsInDistrict(
  districtId: string,
  buildings: Building[],
): number {
  let n = 0;
  for (const bld of buildings) {
    if (bld.districtId === districtId) n += 1;
  }
  return n;
}

/**
 * Pure planner: wall segments along the **built cluster** outline (member
 * footprints + margin), with GATE gaps where inter-district roads cross and
 * water-tile skips. Size-gated:
 *   < 6 buildings  → null (clean meadow)
 *   6–9            → low field-stone kerb (no gates/towers)
 *   >= 10          → full wallStyle fortification
 * Emptiness-gated: built footprint / outline area < WALL_MIN_BUILT_RATIO → null.
 * Corner towers only for roman_wall with >= 14 buildings. Deterministic.
 */
export function planDistrictWall(
  district: District,
  roads: Road[],
  buildings: Building[],
  options: WallPlanOptions = {},
): WallPlan | null {
  const style = resolveWallStyle(district.wallStyle);
  if (!style) return null;

  // Size threshold from actual building membership — not reserved bounds area.
  const buildingCount = countBuildingsInDistrict(district.districtId, buildings);
  if (buildingCount < WALL_MIN_LOW) return null;
  const variant: WallPlanVariant =
    buildingCount < WALL_MIN_FULL ? "low" : "full";

  // Outline from real building footprints, not district.bounds (reserved box).
  const b = builtOutlineBounds(district.districtId, buildings);
  if (!b) return null;

  // Emptiness: a wall around mostly empty land is a lying fence.
  const density = builtToEnclosedRatio(district.districtId, buildings, b);
  if (density < WALL_MIN_BUILT_RATIO) return null;

  let byId = options.buildingsById;
  if (!byId) {
    byId = new Map<string, Building>();
    for (const bld of buildings) byId.set(bld.fileId, bld);
  }

  // Full walls punch road gates; low rural kerbs are continuous (no gates).
  let gates: WallGate[] = [];
  if (variant === "full") {
    gates = findRoadGates(b, district.districtId, roads, byId);
    if (gates.length === 0) {
      gates = [fallbackGate(b, options.cityCenter, options.waterTiles)];
    }
  }

  const segments: WallSegment[] = [];
  for (let side = 0; side < 4; side++) {
    segments.push(
      ...planEdgeSegments(b, side as 0 | 1 | 2 | 3, gates, options.waterTiles),
    );
  }

  // Corner towers: roman_wall only, and only on large districts (>= 14).
  const towers: WallTower[] = [];
  if (
    variant === "full" &&
    style === "roman_wall" &&
    buildingCount >= WALL_MIN_TOWERS
  ) {
    const corners = boundsCorners(b);
    for (let i = 0; i < 4; i++) {
      towers.push({ x: corners[i].x, y: corners[i].y, corner: i as 0 | 1 | 2 | 3 });
    }
  }

  // Stable order for determinism checks.
  segments.sort((a, b) =>
    a.side !== b.side
      ? a.side - b.side
      : a.ax !== b.ax
        ? a.ax - b.ax
        : a.ay !== b.ay
          ? a.ay - b.ay
          : a.bx !== b.bx
            ? a.bx - b.bx
            : a.by - b.by,
  );

  return {
    districtId: district.districtId,
    style,
    variant,
    segments,
    towers,
    gates,
  };
}

/**
 * Build a water tile key set from terrain water tiles (if any).
 */
export function waterTileSet(
  water: { gx: number; gy: number }[] | undefined,
): Set<string> {
  const s = new Set<string>();
  if (!water) return s;
  for (const t of water) s.add(`${t.gx},${t.gy}`);
  return s;
}

/**
 * City centre from buildings (mean of coords). Falls back to {0,0} if empty.
 */
export function cityCenterFromBuildings(
  buildings: { coords: { x: number; y: number } }[],
): { x: number; y: number } {
  if (buildings.length === 0) return { x: 0, y: 0 };
  let sx = 0;
  let sy = 0;
  for (const b of buildings) {
    sx += b.coords.x;
    sy += b.coords.y;
  }
  return { x: sx / buildings.length, y: sy / buildings.length };
}

// ---------------------------------------------------------------------------
// Road visual style mapper (pure) — used by the road draw path.
// ---------------------------------------------------------------------------

/**
 * Visual branch for a road SEGMENT on the wire.
 *
 * Backend wire (scanner.rs clone emit):
 *   type: "clone", style: "terra_battuta", provenance: "ast", weight: 1
 * Semantic roads also use style terra_battuta but type/provenance "semantic".
 * Clone twin → dirt path; semantic → faint dashed; **urban** trunk import →
 * limestone cobble with edge stones; **urban** minor → paved street;
 * **rural** (any weight, crossing unbuilt ground) → country track.
 *
 * Context (urban vs rural) is a second axis derived from proximity to built
 * fabric — see {@link isSegmentUrban}. Weight/shared alone must NOT force
 * bright paving across empty meadow (that is the lattice defect STEP 4 fixes).
 */
export type RoadVisualKind =
  | "dirt_dashed"
  | "semantic_faint"
  | "cobble_edged"
  | "cobble"
  | "urban_street"
  | "country_track";

/**
 * Extra tile pad beyond {@link builtOutlineBounds} when classifying a segment
 * as urban. 0 — {@link WALL_OUTLINE_MARGIN} already expands the AABB by 1 tile;
 * a larger pad swallowed inter-district meadow corridors (GAP=3 layout) and
 * left almost no country tracks. Adjacent fabric still hits the margined box.
 */
export const ROAD_URBAN_PAD = 0;

/** Road surface geometry (world / iso px at scale 1). */
export const ROAD_GEOMETRY = {
  /** Urban minor street body width — area, not a hairline. */
  urbanStreetWidth: 7,
  /** Country track body width — narrow beaten earth. */
  countryTrackWidth: 3.2,
  /** Urban end-cap / corner disc radius (≈ half street width). */
  urbanCapRadius: 3.5,
  /** Multi-route urban hub disc radius (corners + T-junctions). */
  urbanHubRadius: 4.5,
} as const;

/**
 * Collect per-district built outlines (padded) for urban/rural segment
 * classification. Deterministic: district order follows first-seen building
 * order in the input array. Empty when there are no buildings.
 */
export function collectBuiltOutlines(
  buildings: readonly Building[],
  pad: number = ROAD_URBAN_PAD,
): Bounds[] {
  const seen = new Set<string>();
  const out: Bounds[] = [];
  for (const b of buildings) {
    if (seen.has(b.districtId)) continue;
    seen.add(b.districtId);
    const base = builtOutlineBounds(b.districtId, buildings as Building[]);
    if (!base) continue;
    out.push({
      x: base.x - pad,
      y: base.y - pad,
      w: base.w + 2 * pad,
      h: base.h + 2 * pad,
    });
  }
  return out;
}

/** Inclusive AABB hit test in cart tile space. */
export function pointInBounds(x: number, y: number, b: Bounds): boolean {
  return x >= b.x && y >= b.y && x <= b.x + b.w && y <= b.y + b.h;
}

/**
 * Whether a cart-space road segment runs through/adjacent to built fabric.
 * Samples endpoints + midpoint against precomputed outlines — pure, O(outlines).
 * A segment far from every district outline is rural.
 */
export function isSegmentUrban(
  a: { x: number; y: number },
  b: { x: number; y: number },
  outlines: readonly Bounds[],
): boolean {
  if (outlines.length === 0) return false;
  const mx = (a.x + b.x) * 0.5;
  const my = (a.y + b.y) * 0.5;
  for (let i = 0; i < outlines.length; i++) {
    const o = outlines[i];
    if (pointInBounds(a.x, a.y, o)) return true;
    if (pointInBounds(b.x, b.y, o)) return true;
    if (pointInBounds(mx, my, o)) return true;
  }
  return false;
}

export function mapRoadVisualKind(
  road: {
    type: string;
    style: string;
    provenance?: string | null;
    weight: number;
  },
  isTrunk: boolean,
  /** Segment context: through/adjacent to built fabric. Default true preserves
   *  legacy call sites that only knew weight/shared hierarchy. */
  isUrban: boolean = true,
): RoadVisualKind {
  // Semantic first — backend also tags them terra_battuta.
  if (road.type === "semantic" || road.provenance === "semantic") {
    return "semantic_faint";
  }
  // Clone twin roads (P4.2): Rust emits type "clone" + style terra_battuta +
  // provenance "ast". Also accept bare terra_battuta (non-semantic) dirt paths.
  if (road.type === "clone" || road.style === "terra_battuta") {
    return "dirt_dashed";
  }
  // Rural segments: country track regardless of import weight — never bright
  // limestone lattice over empty meadow.
  if (!isUrban) {
    return "country_track";
  }
  // Urban trunk import / lastricata avenues get cobble + edge stones.
  if (isTrunk) {
    if (road.type === "import" || road.style === "lastricata") {
      return "cobble_edged";
    }
    return "cobble";
  }
  return "urban_street";
}

// ---------------------------------------------------------------------------
// Drawing — static, into a shared Graphics (chunk-ops returned for rotation).
// Filled 2.5D bands with real mass (height + thickness + top + shaded side),
// readable at viewport scales 0.3–0.9; alpha near opaque so walls don't
// dissolve against meadow/dirt. A 4px hairline is a fence — we draw masonry.
// ---------------------------------------------------------------------------

/**
 * Overall wall opacity — sits INTO the scene (~0.8), not on top of it.
 * Low rural boundary is intentionally softer.
 */
const WALL_ALPHA = {
  body: 0.82,
  detail: 0.8,
  top: 0.86,
  tower: 0.84,
  low: 0.72,
} as const;

const BAND_W = WALL_GEOMETRY.bandW;
const WALL_H = WALL_GEOMETRY.wallH;
const LOW_BAND_W = WALL_GEOMETRY.lowBandW;
const LOW_WALL_H = WALL_GEOMETRY.lowWallH;

/** Deterministic 0..1 hash from screen/cart position (no Math.random). */
function posHash(x: number, y: number): number {
  // Integer lattice so the same stake site always yields the same jitter.
  const ix = Math.floor(x * 10);
  const iy = Math.floor(y * 10);
  let h = (ix * 374761393 + iy * 668265263) | 0;
  h = Math.imul(h ^ (h >>> 13), 1274126177);
  return ((h ^ (h >>> 16)) >>> 0) / 4294967296;
}

/** Unit tangent + perpendicular along an iso edge. */
function edgeFrame(
  from: IsoPoint,
  to: IsoPoint,
): { len: number; ux: number; uy: number; nx: number; ny: number } {
  const len = dist(from, to) || 1;
  const ux = (to.x - from.x) / len;
  const uy = (to.y - from.y) / len;
  return { len, ux, uy, nx: -uy, ny: ux };
}

/**
 * Body colour by cart edge side under kit sun SUN.dir = "NW":
 * N/W faces catch light; E/S sit in shade (same language as kit face factors).
 */
function wallBodyForSide(
  side: 0 | 1 | 2 | 3,
  lit: number,
  shade: number,
): number {
  // 0=N lit, 1=E shade, 2=S shade, 3=W lit
  return side === 1 || side === 2 ? shade : lit;
}

/**
 * Classic city-builder wall body: front face extruded screen-up + lighter top
 * band + shaded SE side slab, grounded by a soft contact shadow (same alpha
 * contract as buildings/props via CONTACT_SHADOW).
 * Returns fill/stroke-op count.
 */
function drawWallBand(
  g: Graphics,
  from: IsoPoint,
  to: IsoPoint,
  bodyColor: number,
  topColor: number,
  bottomColor: number,
  alpha: number,
  width: number = BAND_W,
  height: number = WALL_H,
): number {
  const { len, nx, ny } = edgeFrame(from, to);
  if (len < 0.5) return 0;
  const hw = width / 2;

  // Soft contact pool under the wall — unified with buildings/props (α 0.3).
  g.moveTo(from.x, from.y + 1.5)
    .lineTo(to.x, to.y + 1.5)
    .stroke({
      color: bottomColor,
      alpha: CONTACT_SHADOW.alpha * alpha,
      width: Math.max(2.5, width * 0.55),
    });

  // Front face — vertical band (screen-up = height). Mass, not a hairline.
  g.poly([
    from.x,
    from.y + 0.5,
    to.x,
    to.y + 0.5,
    to.x,
    to.y - height,
    from.x,
    from.y - height,
  ]).fill({ color: bodyColor, alpha });

  // Shaded thickness slab on the SE-ish normal (reads as solid volume).
  // Prefer the perpendicular that points down-screen so the side is visible.
  const sideNx = ny > 0 ? nx : -nx;
  const sideNy = ny > 0 ? ny : -ny;
  const sideDepth = Math.max(2.5, width * 0.45);
  g.poly([
    from.x,
    from.y + 0.5,
    to.x,
    to.y + 0.5,
    to.x + sideNx * sideDepth,
    to.y + 0.5 + sideNy * sideDepth,
    from.x + sideNx * sideDepth,
    from.y + 0.5 + sideNy * sideDepth,
  ]).fill({ color: bottomColor, alpha: alpha * 0.85 });

  // Lighter top face — same thickness, raised by `height` (walkable parapet).
  g.poly([
    from.x + nx * hw,
    from.y - height + ny * hw,
    to.x + nx * hw,
    to.y - height + ny * hw,
    to.x - nx * hw,
    to.y - height - ny * hw,
    from.x - nx * hw,
    from.y - height - ny * hw,
  ]).fill({ color: topColor, alpha: Math.min(1, alpha + 0.04) });

  return 4;
}

/** Draw a dashed stroke between two iso points (short solid runs). */
function strokeDashed(
  g: Graphics,
  from: IsoPoint,
  to: IsoPoint,
  color: number,
  alpha: number,
  width: number,
  dashLen: number,
  gapLen: number,
): number {
  const total = dist(from, to);
  if (total < 0.5) return 0;
  const step = dashLen + gapLen;
  const n = Math.max(1, Math.floor(total / step));
  let ops = 0;
  for (let i = 0; i <= n; i++) {
    const t0 = (i * step) / total;
    if (t0 >= 1) break;
    const t1 = Math.min(1, (i * step + dashLen) / total);
    const a = lerp(from, to, t0);
    const b = lerp(from, to, t1);
    g.moveTo(a.x, a.y).lineTo(b.x, b.y);
    g.stroke({ color, alpha, width });
    ops += 1;
  }
  return ops;
}

/** Roman sandstone wall: mass band + sparse merlons along the top. */
function drawRomanSegment(
  g: Graphics,
  from: IsoPoint,
  to: IsoPoint,
  merlonSpacing: number,
  side: 0 | 1 | 2 | 3,
): number {
  const body = wallBodyForSide(
    side,
    DERIVED.wallStone,
    DERIVED.wallStoneDark,
  );
  let ops = drawWallBand(
    g,
    from,
    to,
    body,
    DERIVED.wallStoneLight,
    DERIVED.wallStoneDark,
    WALL_ALPHA.body,
    BAND_W,
    WALL_H,
  );
  if (ops === 0) return 0;

  // Merlons with deterministic height jitter so the rhythm is not Lego-grid.
  const total = dist(from, to);
  const spacing = merlonSpacing;
  const steps = Math.max(0, Math.floor(total / spacing));
  const blockW = WALL_GEOMETRY.merlonW;
  const blockH = WALL_GEOMETRY.merlonH;
  for (let i = 1; i < steps; i++) {
    const p = lerp(from, to, i / steps);
    // Height jitter 0..2 px from position hash (deterministic).
    const hJitter = Math.floor(posHash(p.x, p.y) * 3);
    const bh = blockH + hJitter;
    g.rect(p.x - blockW / 2, p.y - WALL_H - bh, blockW, bh).fill({
      color: DERIVED.crenellation,
      alpha: WALL_ALPHA.detail,
    });
    // Lit lip on the merlon (top surface under NW sun).
    g.rect(p.x - blockW / 2, p.y - WALL_H - bh, blockW, 1.5).fill({
      color: DERIVED.wallStoneLight,
      alpha: WALL_ALPHA.top,
    });
    ops += 2;
  }
  return ops;
}

/** Corner tower: chunky square with plinth + cap (roman_wall). */
function drawRomanTower(g: Graphics, c: IsoPoint): number {
  const s = WALL_GEOMETRY.towerSize;
  const half = s / 2;
  const h = WALL_GEOMETRY.towerH;
  // Body.
  g.rect(c.x - half, c.y - h + 1, s, h).fill({
    color: DERIVED.wallStone,
    alpha: WALL_ALPHA.tower,
  });
  // Darker SE half-face (NW sun shade).
  g.rect(c.x, c.y - h + 1, half, h).fill({
    color: DERIVED.wallStoneDark,
    alpha: WALL_ALPHA.tower * 0.9,
  });
  // Darker lower band (base plinth).
  g.rect(c.x - half, c.y - 2, s, 4).fill({
    color: DERIVED.wallStoneDark,
    alpha: WALL_ALPHA.tower,
  });
  // Cap slab.
  g.rect(c.x - half - 1, c.y - h - 2, s + 2, 4).fill({
    color: DERIVED.wallStoneDark,
    alpha: WALL_ALPHA.tower,
  });
  // Cap highlight (top surface).
  g.rect(c.x - half - 1, c.y - h - 2, s + 2, 1.6).fill({
    color: DERIVED.wallStoneLight,
    alpha: WALL_ALPHA.top,
  });
  // Contact shadow under tower.
  g.ellipse(c.x, c.y + 1.5, s * 0.55, s * 0.22).fill({
    color: DERIVED.wallStoneDark,
    alpha: CONTACT_SHADOW.alpha,
  });
  return 6;
}

/**
 * Gate jamb blocks — thicker stone pillars on both sides of a clear gap.
 * Cart-space offset along the edge by GATE_HALF from the gate centre.
 */
function drawGateJambs(g: Graphics, gate: WallGate): number {
  // Edge unit in cart space (CW bounds walk).
  const tCart =
    gate.side === 0
      ? { x: 1, y: 0 }
      : gate.side === 1
        ? { x: 0, y: 1 }
        : gate.side === 2
          ? { x: -1, y: 0 }
          : { x: 0, y: -1 };
  const j0 = cartToIso(gate.x - tCart.x * GATE_HALF, gate.y - tCart.y * GATE_HALF);
  const j1 = cartToIso(gate.x + tCart.x * GATE_HALF, gate.y + tCart.y * GATE_HALF);
  let ops = 0;
  const w = WALL_GEOMETRY.jambW;
  const h = WALL_GEOMETRY.jambH;
  for (const p of [j0, j1]) {
    g.rect(p.x - w / 2, p.y - h + 1, w, h).fill({
      color: DERIVED.wallStoneDark,
      alpha: WALL_ALPHA.body,
    });
    g.rect(p.x - w / 2, p.y - h + 1, w, 2.2).fill({
      color: DERIVED.wallStoneLight,
      alpha: WALL_ALPHA.top,
    });
    ops += 2;
  }
  return ops;
}

/**
 * Palisade as a wooden STOCKADE with mass — solid timber band + sparse taller
 * posts — not a 1.8px picket fence. Reserved for districts whose wire style is
 * still "palisade" (frontier/outer); Zeus core fabric is roman_wall.
 */
function drawPalisadeSegment(
  g: Graphics,
  from: IsoPoint,
  to: IsoPoint,
  stakeSpacing: number,
  side: 0 | 1 | 2 | 3,
): number {
  const total = dist(from, to);
  if (total < 0.5) return 0;

  const body = wallBodyForSide(side, DERIVED.wallWood, DERIVED.wallWoodDark);
  // Solid timber curtain wall first (mass).
  let ops = drawWallBand(
    g,
    from,
    to,
    body,
    DERIVED.wallWood,
    DERIVED.wallWoodDark,
    WALL_ALPHA.body,
    BAND_W - 1,
    WALL_H - 2,
  );
  if (ops === 0) return 0;

  // Sparse taller posts on top of the curtain (stockade silhouette).
  const spacing = Math.max(stakeSpacing, 14);
  const steps = Math.max(1, Math.floor(total / spacing));
  const stakeW = 3.2;
  const baseH = WALL_H + 3;
  for (let i = 0; i <= steps; i++) {
    const p = lerp(from, to, i / steps);
    const jitter = posHash(p.x, p.y) * 2.5 - 0.2;
    const h = baseH + jitter;
    const wood = i % 2 === 0 ? DERIVED.wallWood : DERIVED.wallWoodDark;
    g.rect(p.x - stakeW / 2, p.y - h, stakeW, h + 0.8).fill({
      color: wood,
      alpha: WALL_ALPHA.body,
    });
    // Pointed tip.
    g.rect(p.x - stakeW / 2, p.y - h - 2, stakeW, 2.5).fill({
      color: DERIVED.wallWood,
      alpha: WALL_ALPHA.top,
    });
    ops += 2;
  }
  return ops;
}

/**
 * Aqueduct: filled channel band on top + pier rects with arch gaps below.
 * Warm sandstone family (same tokens as roman walls). Mass-matched piers.
 */
function drawAqueductSegment(g: Graphics, from: IsoPoint, to: IsoPoint): number {
  const total = dist(from, to);
  if (total < 0.5) return 0;

  const archSpan = 18;
  const pierW = 5;
  const pierH = 14;
  const channelH = 5;
  const channelY = pierH; // channel sits on top of piers
  let ops = 0;

  // Piers at regular intervals (gaps between = arches).
  const arches = Math.max(1, Math.floor(total / archSpan));
  for (let i = 0; i <= arches; i++) {
    const p = lerp(from, to, i / arches);
    g.rect(p.x - pierW / 2, p.y - pierH + 1, pierW, pierH).fill({
      color: DERIVED.wallAqueductDark,
      alpha: WALL_ALPHA.body,
    });
    // Lit face strip on pier (NW sun).
    g.rect(p.x - pierW / 2, p.y - pierH + 1, pierW * 0.4, pierH).fill({
      color: DERIVED.wallAqueduct,
      alpha: WALL_ALPHA.detail * 0.75,
    });
    ops += 2;
  }

  // Channel band on top (lighter fill).
  const chFrom: IsoPoint = { x: from.x, y: from.y - channelY };
  const chTo: IsoPoint = { x: to.x, y: to.y - channelY };
  ops += drawWallBand(
    g,
    chFrom,
    chTo,
    DERIVED.wallAqueduct,
    DERIVED.wallStoneLight,
    DERIVED.wallAqueductDark,
    WALL_ALPHA.body,
    BAND_W + 0.5,
    channelH,
  );
  return ops;
}

/**
 * Low rural boundary (6–9 buildings): short field-stone kerb with real mass
 * (not a 1.3px hairline). Alpha softer than full fortification.
 */
function drawLowBoundarySegment(
  g: Graphics,
  from: IsoPoint,
  to: IsoPoint,
  side: 0 | 1 | 2 | 3,
): number {
  const total = dist(from, to);
  if (total < 0.5) return 0;
  const body = wallBodyForSide(
    side,
    DERIVED.wallStone,
    DERIVED.wallStoneDark,
  );
  let ops = drawWallBand(
    g,
    from,
    to,
    body,
    DERIVED.wallStoneLight,
    DERIVED.wallStoneDark,
    WALL_ALPHA.low,
    LOW_BAND_W,
    LOW_WALL_H,
  );

  // Occasional cap stones every ~22px along the kerb.
  const DOT_SPACING = 22;
  const steps = Math.max(0, Math.floor(total / DOT_SPACING));
  for (let i = 1; i < steps; i++) {
    const p = lerp(from, to, i / steps);
    const s = 2.2 + posHash(p.x, p.y) * 1.2;
    g.rect(p.x - s / 2, p.y - LOW_WALL_H - s * 0.4, s, s * 0.7).fill({
      color: i % 2 === 0 ? DERIVED.wallStoneDark : DERIVED.wallStoneLight,
      alpha: WALL_ALPHA.low,
    });
    ops += 1;
  }
  return ops;
}

/** Base merlon / stake spacing (world px). Scaled up on long perimeters. */
const BASE_MERLON_SPACING = 28; // sparse ~26–32 range
const BASE_STAKE_SPACING = 9; // fewer stakes than before
/**
 * Cap detail (merlon/stake) ops per district. Each detail unit costs ~2 ops;
 * band/towers/gates add a small fixed overhead. Target total ops/district ≈ 400.
 */
const MAX_DETAIL_OPS_PER_DISTRICT = 320;

/**
 * Iso-space length of planned wall segments (gates/water already punched).
 * Used to scale merlon/stake density on large districts — zoom-independent.
 */
function planIsoPerimeter(plan: WallPlan): number {
  let total = 0;
  for (const seg of plan.segments) {
    total += dist(cartToIso(seg.ax, seg.ay), cartToIso(seg.bx, seg.by));
  }
  return total;
}

/**
 * Increase base spacing when perimeter would exceed the detail-ops budget.
 * spacing' = max(base, 2 * peri / MAX_DETAIL_OPS) so ~2 ops/unit stays under budget.
 */
function detailSpacing(base: number, isoPerimeter: number): number {
  if (isoPerimeter <= 0) return base;
  const minForBudget = (2 * isoPerimeter) / MAX_DETAIL_OPS_PER_DISTRICT;
  return minForBudget > base ? minForBudget : base;
}

/**
 * Draw a planned wall into `g`. Returns the number of fill/stroke ops so the
 * caller can rotate Graphics chunks (~300 ops for walls).
 * Low-variant plans draw a rural kerb regardless of wallStyle.
 */
export function drawWallPlan(g: Graphics, plan: WallPlan): number {
  let ops = 0;

  // Rural low boundary: field-stone kerb with mass, no fortification details.
  if (plan.variant === "low") {
    for (const seg of plan.segments) {
      ops += drawLowBoundarySegment(
        g,
        cartToIso(seg.ax, seg.ay),
        cartToIso(seg.bx, seg.by),
        seg.side,
      );
    }
    return ops;
  }

  const peri = planIsoPerimeter(plan);
  const merlonSp = detailSpacing(BASE_MERLON_SPACING, peri);
  const stakeSp = detailSpacing(BASE_STAKE_SPACING, peri);
  for (const seg of plan.segments) {
    const from = cartToIso(seg.ax, seg.ay);
    const to = cartToIso(seg.bx, seg.by);
    if (plan.style === "roman_wall") {
      ops += drawRomanSegment(g, from, to, merlonSp, seg.side);
    } else if (plan.style === "palisade") {
      ops += drawPalisadeSegment(g, from, to, stakeSp, seg.side);
    } else {
      ops += drawAqueductSegment(g, from, to);
    }
  }
  if (plan.style === "roman_wall") {
    for (const t of plan.towers) {
      ops += drawRomanTower(g, cartToIso(t.x, t.y));
    }
    for (const gate of plan.gates) {
      ops += drawGateJambs(g, gate);
    }
  }
  return ops;
}

/**
 * Draw a dirt (terra battuta / clone) dashed earthy path. Returns stroke ops.
 */
export function drawDirtDashed(
  g: Graphics,
  from: IsoPoint,
  to: IsoPoint,
  alphaMult = 1,
): number {
  return strokeDashed(
    g,
    from,
    to,
    DERIVED.groundDirt,
    0.38 * alphaMult,
    1.6,
    6,
    4,
  );
}

/**
 * Draw a semantic faint dashed line (weaker than import minor). Returns ops.
 */
export function drawSemanticFaint(
  g: Graphics,
  from: IsoPoint,
  to: IsoPoint,
  alphaMult = 1,
): number {
  return strokeDashed(
    g,
    from,
    to,
    DERIVED.wallStoneDark,
    0.18 * alphaMult,
    1.2,
    5,
    6,
  );
}

/**
 * Extra edge-stone outlines for trunk import roads (two thin darker parallels).
 * Call AFTER the cobble body. Returns stroke ops.
 *
 * Each stroke is extended ~2px past the segment endpoints along the segment
 * direction so adjacent polyline segments meet (cheap miter approximation —
 * without this, non-collinear corners leave a visible gap).
 */
export function drawTrunkEdgeStones(
  g: Graphics,
  from: IsoPoint,
  to: IsoPoint,
  roadWidth: number,
  alphaMult = 1,
): number {
  const total = dist(from, to) || 1;
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  const ux = dx / total;
  const uy = dy / total;
  const px = (-dy / total) * (roadWidth / 2 + 0.8);
  const py = (dx / total) * (roadWidth / 2 + 0.8);
  // Extend past endpoints so corner joints close (~2px miter fudge).
  const EXT = 2;
  const ax = from.x - ux * EXT;
  const ay = from.y - uy * EXT;
  const bx = to.x + ux * EXT;
  const by = to.y + uy * EXT;
  g.moveTo(ax + px, ay + py).lineTo(bx + px, by + py);
  g.moveTo(ax - px, ay - py).lineTo(bx - px, by - py);
  g.stroke({
    color: DERIVED.wallStoneDark,
    alpha: 0.22 * alphaMult,
    width: 1,
  });
  return 1;
}

/** Alphas for STEP-4 road surfaces (flat per class — no stack darkening). */
export const ROAD_SURFACE_ALPHA = {
  urbanFill: 0.72,
  urbanKerb: 0.32,
  urbanCap: 0.7,
  countryFill: 0.38,
  countryEdge: 0.18,
} as const;

/**
 * Country track: narrow beaten-earth strip, soft edges, no kerb. Recedes into
 * meadow; still a traceable surface (not a 2px fence hairline). Returns ops.
 */
export function drawCountryTrack(
  g: Graphics,
  from: IsoPoint,
  to: IsoPoint,
  alphaMult = 1,
  width: number = ROAD_GEOMETRY.countryTrackWidth,
): number {
  const total = dist(from, to) || 1;
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  const half = width / 2;
  const px = (-dy / total) * half;
  const py = (dx / total) * half;
  g.poly([
    from.x + px,
    from.y + py,
    to.x + px,
    to.y + py,
    to.x - px,
    to.y - py,
    from.x - px,
    from.y - py,
  ]).fill({
    color: DERIVED.roadCountryDirt,
    alpha: ROAD_SURFACE_ALPHA.countryFill * alphaMult,
  });
  // Soft outer whisper (no hard kerb stroke).
  g.moveTo(from.x + px, from.y + py).lineTo(to.x + px, to.y + py);
  g.moveTo(from.x - px, from.y - py).lineTo(to.x - px, to.y - py);
  g.stroke({
    color: DERIVED.roadCountryDirtSoft,
    alpha: ROAD_SURFACE_ALPHA.countryEdge * alphaMult,
    width: 1,
  });
  return 2; // fill + stroke
}

/**
 * Urban minor street: limestone pave strip with kerb + end caps so corners
 * and T-junctions read as connected pavement (not crossed strokes). Returns ops.
 */
export function drawUrbanStreet(
  g: Graphics,
  from: IsoPoint,
  to: IsoPoint,
  alphaMult = 1,
  width: number = ROAD_GEOMETRY.urbanStreetWidth,
): number {
  const total = dist(from, to) || 1;
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  const half = width / 2;
  const px = (-dy / total) * half;
  const py = (dx / total) * half;
  g.poly([
    from.x + px,
    from.y + py,
    to.x + px,
    to.y + py,
    to.x - px,
    to.y - py,
    from.x - px,
    from.y - py,
  ]).fill({
    color: DERIVED.roadUrbanPave,
    alpha: ROAD_SURFACE_ALPHA.urbanFill * alphaMult,
  });
  g.moveTo(from.x + px, from.y + py).lineTo(to.x + px, to.y + py);
  g.moveTo(from.x - px, from.y - py).lineTo(to.x - px, to.y - py);
  g.stroke({
    color: DERIVED.roadUrbanKerb,
    alpha: ROAD_SURFACE_ALPHA.urbanKerb * alphaMult,
    width: 1,
  });
  // End caps close miter gaps at corners / T-junctions.
  const r = Math.min(ROAD_GEOMETRY.urbanCapRadius, half);
  g.circle(from.x, from.y, r).fill({
    color: DERIVED.roadUrbanPaveAlt,
    alpha: ROAD_SURFACE_ALPHA.urbanCap * alphaMult,
  });
  g.circle(to.x, to.y, r).fill({
    color: DERIVED.roadUrbanPaveAlt,
    alpha: ROAD_SURFACE_ALPHA.urbanCap * alphaMult,
  });
  return 4; // fill + kerb stroke + 2 caps
}

/**
 * Urban multi-route hub disc (corners n=2, T n=3, true hubs n≥4).
 * Drawn on the urban/trunk layer after segments. Returns ops.
 */
export function drawUrbanHub(
  g: Graphics,
  x: number,
  y: number,
  n: number,
  alphaMult = 1,
): number {
  // Corners and T-junctions (n≥2) get a pavement disc; rural-only kinks stay bare.
  if (n < 2) return 0;
  const r =
    n >= 4
      ? ROAD_GEOMETRY.urbanHubRadius + 1
      : n >= 3
        ? ROAD_GEOMETRY.urbanHubRadius
        : ROAD_GEOMETRY.urbanCapRadius;
  g.circle(x, y, r).fill({
    color: DERIVED.roadUrbanPaveAlt,
    alpha: ROAD_SURFACE_ALPHA.urbanCap * alphaMult,
  });
  return 1;
}
