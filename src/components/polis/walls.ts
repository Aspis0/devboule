// District walls — pure planner + static drawer for Polis F3 wall styles.
//
// The backend already emits District.wallStyle ("roman_wall" | "aqueduct" |
// "palisade" | "none"); this module is the FIRST renderer code that reads it.
// Walls sit ON the district bounds diamond as thin warm sandstone 2.5D bands
// (Caesar III sand/stucco, not cold grey Lego). Readable at viewport scales
// 0.3–0.9 but subordinate to buildings (alpha ~0.8). Baked once into Graphics;
// redrawn only when districts rebuild.
//
// SIZE GATE (by building count in district, not bounds):
//   < 6  → null (clean meadow)
//   6–9  → low field-stone kerb / hedge (variant "low")
//   >=10 → full wallStyle fortification (variant "full")
// Corner towers: roman_wall only, and only when building count >= 14.
//
// DETERMINISM: same (district, roads, buildings, water) → identical segments.
// No Math.random(). Gate placement is derived from road crossings or a single
// city-center-facing fallback gate. Merlon/stake jitter uses a position hash.
// HONESTY: walls decorate the bounds the backend already computed — never invent
// a district or a road.

import { Graphics } from "pixi.js";
import type { Bounds, Building, District, Road, WallStyle } from "../../types/city";
import { cartToIso, dist, lerp, type IsoPoint } from "./iso";
import { DERIVED } from "./palette";

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
 * Collect gate centres on the district boundary where inter-district roads
 * cross. Prefers path polylines; falls back to straight from→to via buildings.
 * Sorted deterministically (side, then t).
 *
 * Matches `drawRoads`: roads whose endpoints are missing from `byId` are
 * skipped entirely (stale refs must not punch phantom gates).
 */
function findRoadGates(
  district: District,
  roads: Road[],
  byId: Map<string, Building>,
): WallGate[] {
  const b = district.bounds;
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
    const fromOut = fromB.districtId !== district.districtId;
    const toOut = toB.districtId !== district.districtId;
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
 * is missing / coincides with district centre). Probes several t values so the
 * gate prefers dry land when the midpoint sits on water.
 */
function fallbackGate(
  district: District,
  cityCenter: { x: number; y: number } | undefined,
  water: Set<string> | undefined,
): WallGate {
  const b = district.bounds;
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
 * Pure planner: wall segments along the district bounds diamond, with GATE
 * gaps where inter-district roads cross and water-tile skips. Size-gated:
 *   < 6 buildings  → null (clean meadow)
 *   6–9            → low field-stone kerb (no gates/towers)
 *   >= 10          → full wallStyle fortification
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
  const b = district.bounds;
  // Reject zero / negative / non-finite bounds (malformed wire data).
  if (!(Number.isFinite(b.w) && Number.isFinite(b.h)) || b.w <= 0 || b.h <= 0) {
    return null;
  }

  // Size threshold from actual building membership — not bounds area.
  const buildingCount = countBuildingsInDistrict(district.districtId, buildings);
  if (buildingCount < WALL_MIN_LOW) return null;
  const variant: WallPlanVariant =
    buildingCount < WALL_MIN_FULL ? "low" : "full";

  let byId = options.buildingsById;
  if (!byId) {
    byId = new Map<string, Building>();
    for (const bld of buildings) byId.set(bld.fileId, bld);
  }

  // Full walls punch road gates; low rural kerbs are continuous (no gates).
  let gates: WallGate[] = [];
  if (variant === "full") {
    gates = findRoadGates(district, roads, byId);
    if (gates.length === 0) {
      gates = [fallbackGate(district, options.cityCenter, options.waterTiles)];
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
 * Visual branch for a road on the wire.
 *
 * Backend wire (scanner.rs clone emit):
 *   type: "clone", style: "terra_battuta", provenance: "ast", weight: 1
 * Semantic roads also use style terra_battuta but type/provenance "semantic".
 * Clone twin → dirt path; semantic → faint dashed; trunk import → cobble with
 * edge stones; else default minor.
 */
export type RoadVisualKind =
  | "dirt_dashed"
  | "semantic_faint"
  | "cobble_edged"
  | "cobble"
  | "minor";

export function mapRoadVisualKind(
  road: {
    type: string;
    style: string;
    provenance?: string | null;
    weight: number;
  },
  isTrunk: boolean,
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
  // Trunk import / lastricata avenues get cobble + edge stones.
  if (isTrunk) {
    if (road.type === "import" || road.style === "lastricata") {
      return "cobble_edged";
    }
    return "cobble";
  }
  return "minor";
}

// ---------------------------------------------------------------------------
// Drawing — static, into a shared Graphics (chunk-ops returned for rotation).
// Filled 2.5D bands readable at viewport scales 0.3–0.9; alpha near opaque so
// walls don't dissolve against meadow/dirt at world stroke widths.
// ---------------------------------------------------------------------------

/**
 * Overall wall opacity — sits INTO the scene (~0.8), not on top of it.
 * Low rural boundary is intentionally softer.
 */
const WALL_ALPHA = {
  body: 0.8,
  detail: 0.78,
  top: 0.82,
  tower: 0.8,
  low: 0.6,
} as const;

/** Wall band thickness (world/screen px) and fake height (screen-up offset). */
const BAND_W = 4;
const WALL_H = 4;

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
 * Classic city-builder wall body: thin front face extruded screen-up + lighter
 * top face, grounded by a soft dark base line (ground-contact shadow).
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
  width = BAND_W,
  height = WALL_H,
): number {
  const { len, nx, ny } = edgeFrame(from, to);
  if (len < 0.5) return 0;
  const hw = width / 2;

  // Soft dark base line — ground-contact shadow under the wall band.
  g.moveTo(from.x, from.y + 1.2)
    .lineTo(to.x, to.y + 1.2)
    .stroke({ color: bottomColor, alpha: alpha * 0.75, width: 2 });

  // Front face — vertical band (screen-up = height).
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

  // Lighter top face — same thickness, raised by `height`.
  g.poly([
    from.x + nx * hw,
    from.y - height + ny * hw,
    to.x + nx * hw,
    to.y - height + ny * hw,
    to.x - nx * hw,
    to.y - height - ny * hw,
    from.x - nx * hw,
    from.y - height - ny * hw,
  ]).fill({ color: topColor, alpha: Math.min(1, alpha + 0.02) });

  return 3;
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

/** Roman sandstone wall: thin band + small sparse merlons along the top. */
function drawRomanSegment(
  g: Graphics,
  from: IsoPoint,
  to: IsoPoint,
  merlonSpacing: number,
): number {
  let ops = drawWallBand(
    g,
    from,
    to,
    DERIVED.wallStone,
    DERIVED.wallStoneLight,
    DERIVED.wallStoneDark,
    WALL_ALPHA.body,
    BAND_W,
    WALL_H,
  );
  if (ops === 0) return 0;

  // Small sparse merlons (~3×3) with deterministic height jitter so the
  // rhythm is not mechanical Lego blocks. Spacing ~26–32px base.
  const total = dist(from, to);
  const spacing = merlonSpacing;
  const steps = Math.max(0, Math.floor(total / spacing));
  const blockW = 3;
  const blockH = 3;
  for (let i = 1; i < steps; i++) {
    const p = lerp(from, to, i / steps);
    // Height jitter 0..2 px from position hash (deterministic).
    const hJitter = Math.floor(posHash(p.x, p.y) * 3);
    const bh = blockH + hJitter;
    g.rect(p.x - blockW / 2, p.y - WALL_H - bh, blockW, bh).fill({
      color: DERIVED.crenellation,
      alpha: WALL_ALPHA.detail,
    });
    // Slight lit lip on the merlon.
    g.rect(p.x - blockW / 2, p.y - WALL_H - bh, blockW, 1).fill({
      color: DERIVED.wallStoneLight,
      alpha: WALL_ALPHA.top,
    });
    ops += 2;
  }
  return ops;
}

/** Corner tower: compact square ~10 world px with a darker cap (roman_wall). */
function drawRomanTower(g: Graphics, c: IsoPoint): number {
  const s = 10;
  const half = s / 2;
  const h = 8;
  // Body.
  g.rect(c.x - half, c.y - h + 1, s, h).fill({
    color: DERIVED.wallStone,
    alpha: WALL_ALPHA.tower,
  });
  // Darker lower band (base plinth).
  g.rect(c.x - half, c.y - 1.5, s, 3.5).fill({
    color: DERIVED.wallStoneDark,
    alpha: WALL_ALPHA.tower,
  });
  // Darker cap slab.
  g.rect(c.x - half - 0.5, c.y - h - 1.5, s + 1, 3).fill({
    color: DERIVED.wallStoneDark,
    alpha: WALL_ALPHA.tower,
  });
  // Cap highlight.
  g.rect(c.x - half - 0.5, c.y - h - 1.5, s + 1, 1.2).fill({
    color: DERIVED.wallStoneLight,
    alpha: WALL_ALPHA.top,
  });
  return 4;
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
  for (const p of [j0, j1]) {
    const w = 5;
    const h = 7;
    g.rect(p.x - w / 2, p.y - h + 1, w, h).fill({
      color: DERIVED.wallStoneDark,
      alpha: WALL_ALPHA.body,
    });
    g.rect(p.x - w / 2, p.y - h + 1, w, 1.8).fill({
      color: DERIVED.wallStoneLight,
      alpha: WALL_ALPHA.top,
    });
    ops += 2;
  }
  return ops;
}

/** Palisade: fewer, thinner warm-wood stakes with deterministic height jitter. */
function drawPalisadeSegment(
  g: Graphics,
  from: IsoPoint,
  to: IsoPoint,
  stakeSpacing: number,
): number {
  const total = dist(from, to);
  if (total < 0.5) return 0;
  const spacing = stakeSpacing;
  const steps = Math.max(1, Math.floor(total / spacing));
  const stakeW = 1.8;
  const baseH = 7;
  let ops = 0;

  // Soft ground-contact shadow under the stakes.
  g.moveTo(from.x, from.y + 1.2)
    .lineTo(to.x, to.y + 1.2)
    .stroke({ color: DERIVED.wallWoodDark, alpha: WALL_ALPHA.detail * 0.7, width: 1.6 });
  ops += 1;

  for (let i = 0; i <= steps; i++) {
    const p = lerp(from, to, i / steps);
    const jitter = posHash(p.x, p.y) * 2.2 - 0.3; // ~-0.3 .. +1.9
    const h = baseH + jitter;
    const wood = i % 2 === 0 ? DERIVED.wallWood : DERIVED.wallWoodDark;
    g.rect(p.x - stakeW / 2, p.y - h, stakeW, h + 0.8).fill({
      color: wood,
      alpha: WALL_ALPHA.body,
    });
    // Lighter tip.
    g.rect(p.x - stakeW / 2, p.y - h, stakeW, 1.5).fill({
      color: DERIVED.wallWood,
      alpha: WALL_ALPHA.top,
    });
    ops += 2;
  }
  return ops;
}

/**
 * Aqueduct: filled channel band on top + pier rects with arch gaps below.
 * Warm sandstone family (same tokens as roman walls).
 */
function drawAqueductSegment(g: Graphics, from: IsoPoint, to: IsoPoint): number {
  const total = dist(from, to);
  if (total < 0.5) return 0;

  const archSpan = 16;
  const pierW = 4;
  const pierH = 10;
  const channelH = 3.5;
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
    // Lit face strip on pier.
    g.rect(p.x - pierW / 2, p.y - pierH + 1, pierW * 0.4, pierH).fill({
      color: DERIVED.wallAqueduct,
      alpha: WALL_ALPHA.detail * 0.7,
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
 * Low rural boundary (6–9 buildings): thin double-stroke kerb (stone + shadow)
 * with tiny stone dots every ~20px. Reads as a field-stone hedge line, not a
 * fortification. Alpha ~0.6.
 */
function drawLowBoundarySegment(g: Graphics, from: IsoPoint, to: IsoPoint): number {
  const total = dist(from, to);
  if (total < 0.5) return 0;
  let ops = 0;

  // Shadow stroke (ground contact).
  g.moveTo(from.x, from.y + 0.8)
    .lineTo(to.x, to.y + 0.8)
    .stroke({ color: DERIVED.wallStoneDark, alpha: WALL_ALPHA.low, width: 2.2 });
  ops += 1;
  // Stone stroke on top.
  g.moveTo(from.x, from.y)
    .lineTo(to.x, to.y)
    .stroke({ color: DERIVED.wallStone, alpha: WALL_ALPHA.low, width: 1.3 });
  ops += 1;

  // Tiny stone dots every ~20px along the kerb.
  const DOT_SPACING = 20;
  const steps = Math.max(0, Math.floor(total / DOT_SPACING));
  for (let i = 1; i < steps; i++) {
    const p = lerp(from, to, i / steps);
    // Alternate size slightly via hash for organic rural look.
    const s = 1.4 + posHash(p.x, p.y) * 0.8;
    g.rect(p.x - s / 2, p.y - s / 2 - 0.4, s, s).fill({
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

  // Rural low boundary: field-stone kerb, no fortification details.
  if (plan.variant === "low") {
    for (const seg of plan.segments) {
      ops += drawLowBoundarySegment(
        g,
        cartToIso(seg.ax, seg.ay),
        cartToIso(seg.bx, seg.by),
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
      ops += drawRomanSegment(g, from, to, merlonSp);
    } else if (plan.style === "palisade") {
      ops += drawPalisadeSegment(g, from, to, stakeSp);
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
