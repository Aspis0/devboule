// Coast — harbor–sea piers + shoreline decoration (pure planners + draw).
//
// HONESTY: piers only when real water tiles are within reach of a harbor /
// lighthouse building — never invent a sea. Shore props only on sand tiles
// that are adjacent to water. No click targets, no file implication.
//
// DETERMINISM: pier layout seeds from building fileId; shore scatter from
// (gx, gy). No Math.random().

import { Graphics, Container } from "pixi.js";
import { cartToIso } from "./iso";
import { DERIVED } from "./palette";
import { hashCoords, rngFromCoords, rngFromString } from "./rng";
import type { TerrainData } from "../../types/city";

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/** Purpose slugs that receive a sea-glue pier (kit builders: Limen / Pharos). */
export const HARBOR_PURPOSES = new Set(["harbor", "lighthouse"]);

/** Max pier length in tiles (east from the building footprint edge). */
export const PIER_LEN_MIN = 2;
export const PIER_LEN_MAX = 4;
/** How far east we search for water before giving up (no fake sea). */
export const PIER_REACH_MAX = 5;
/** Hard cap on piers drawn city-wide. */
export const MAX_PIERS = 8;

/** Shore scatter: ~1 prop per 3–4 shoreline tiles. */
export const SHORE_DENSITY = 0.28;
/** Hard cap on shoreline decoration items. */
export const MAX_SHORE_PROPS = 180;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface PierBuilding {
  fileId: string;
  purpose: string;
  coords: { x: number; y: number };
}

export interface PierPlan {
  fileId: string;
  /** Building tile (integer). */
  bx: number;
  by: number;
  /** First pier tile east of the building (integer gx). */
  startGx: number;
  /** Pier tiles along +gx (inclusive of startGx). */
  length: number;
  /** Mooring posts along the south side (2–3). */
  posts: number;
  /** Boat moored on the north side of the mid-pier tile. */
  boatSide: -1 | 1;
}

export type ShoreDecorKind = "rock" | "reed" | "foam";

export interface ShoreDecorItem {
  gx: number;
  gy: number;
  kind: ShoreDecorKind;
}

// ---------------------------------------------------------------------------
// Pier planner
// ---------------------------------------------------------------------------

/** Build an O(1) water-tile lookup from terrain.water. */
export function waterTileKeySet(terrain?: TerrainData): Set<string> {
  const set = new Set<string>();
  if (!terrain?.water) return set;
  for (const w of terrain.water) set.add(`${w.gx},${w.gy}`);
  return set;
}

/**
 * Distance in tiles east from the building tile to the nearest water tile
 * within a ±1 gy band, up to PIER_REACH_MAX. Null when no water is in reach.
 *
 * Does not invent sea: requires a real water tile in the sparse frame.
 */
export function pierWaterReach(
  bx: number,
  by: number,
  waterSet: Set<string>,
): number | null {
  if (waterSet.size === 0) return null;
  for (let d = 1; d <= PIER_REACH_MAX; d++) {
    for (let oy = -1; oy <= 1; oy++) {
      if (waterSet.has(`${bx + d},${by + oy}`)) return d;
    }
  }
  return null;
}

/**
 * Plan wooden piers for harbor/lighthouse buildings that can reach real water.
 *
 * - Skips non-harbor purposes and buildings with no water within reach.
 * - Deterministic length/posts from fileId.
 * - Cap MAX_PIERS, nearest-to-water first (then fileId for stability).
 */
export function planPiers(
  buildings: readonly PierBuilding[],
  terrain?: TerrainData,
  maxPiers: number = MAX_PIERS,
): PierPlan[] {
  if (!terrain || !buildings.length) return [];
  const waterSet = waterTileKeySet(terrain);
  if (waterSet.size === 0) return [];

  type Cand = PierPlan & { reach: number };
  const cands: Cand[] = [];

  for (const b of buildings) {
    if (!HARBOR_PURPOSES.has(b.purpose)) continue;
    const bx = Math.round(b.coords.x);
    const by = Math.round(b.coords.y);
    const reach = pierWaterReach(bx, by, waterSet);
    if (reach == null) continue;

    const rng = rngFromString(`pier:${b.fileId}`);
    // Length covers at least the reach distance, clamped to [2,4].
    const rawLen = PIER_LEN_MIN + rng.int(0, PIER_LEN_MAX - PIER_LEN_MIN);
    const length = Math.max(PIER_LEN_MIN, Math.min(PIER_LEN_MAX, Math.max(rawLen, reach)));
    const posts = 2 + rng.int(0, 1); // 2 or 3
    const boatSide: -1 | 1 = rng.bool(0.5) ? -1 : 1;

    cands.push({
      fileId: b.fileId,
      bx,
      by,
      startGx: bx + 1,
      length,
      posts,
      boatSide,
      reach,
    });
  }

  // Nearest harbors first; stable tie-break on fileId.
  cands.sort((a, b) => {
    if (a.reach !== b.reach) return a.reach - b.reach;
    return a.fileId < b.fileId ? -1 : a.fileId > b.fileId ? 1 : 0;
  });

  const cap = Math.max(0, maxPiers);
  return cands.slice(0, cap).map(({ reach: _r, ...plan }) => plan);
}

// ---------------------------------------------------------------------------
// Shoreline scatter planner
// ---------------------------------------------------------------------------

/** True when sand (gx,gy) has at least one cardinal water neighbour. */
export function sandAdjacentToWater(
  gx: number,
  gy: number,
  waterSet: Set<string>,
): boolean {
  return (
    waterSet.has(`${gx - 1},${gy}`) ||
    waterSet.has(`${gx + 1},${gy}`) ||
    waterSet.has(`${gx},${gy - 1}`) ||
    waterSet.has(`${gx},${gy + 1}`)
  );
}

/**
 * Deterministic shoreline decoration on sand tiles that touch water.
 * Density ≈ SHORE_DENSITY; hard-capped at maxProps.
 */
export function planShorelineDecor(
  terrain?: TerrainData,
  maxProps: number = MAX_SHORE_PROPS,
): ShoreDecorItem[] {
  if (!terrain?.sand?.length || !terrain.water?.length) return [];
  const waterSet = waterTileKeySet(terrain);
  if (waterSet.size === 0) return [];

  const cap = Math.max(0, maxProps);
  const out: ShoreDecorItem[] = [];

  // Stable scan order so a cap hit is reproducible (not input-array order).
  const sand = [...terrain.sand].sort((a, b) =>
    a.gy !== b.gy ? a.gy - b.gy : a.gx - b.gx,
  );

  for (const s of sand) {
    if (out.length >= cap) break;
    if (!sandAdjacentToWater(s.gx, s.gy, waterSet)) continue;

    const h = hashCoords(s.gx, s.gy);
    // Density gate: ~1 / 3.5 shoreline tiles.
    if ((h % 1000) / 1000 >= SHORE_DENSITY) continue;

    const kindRoll = (h >>> 8) % 10;
    // Mix: rocks common, reeds common, foam sparse accents.
    const kind: ShoreDecorKind =
      kindRoll < 4 ? "rock" : kindRoll < 8 ? "reed" : "foam";

    out.push({ gx: s.gx, gy: s.gy, kind });
  }

  return out;
}

// ---------------------------------------------------------------------------
// Draw — piers
// ---------------------------------------------------------------------------

/**
 * Draw a planked wooden pier + mooring posts + rowboat into `g`.
 * Uses DERIVED.bridgeWood* (palette-derived timber, same family as bridges).
 */
export function drawPier(g: Graphics, plan: PierPlan): void {
  const rng = rngFromString(`pier-draw:${plan.fileId}`);
  const gy = plan.by;

  // Walkway: one deck diamond per pier tile, slightly lifted.
  for (let i = 0; i < plan.length; i++) {
    const gx = plan.startGx + i;
    const c = cartToIso(gx + 0.5, gy + 0.5);
    const hw = 22;
    const hh = 11;
    const lift = 2;
    // Shadow under deck.
    g.poly([
      c.x, c.y - hh + lift + 3,
      c.x + hw, c.y + lift + 3,
      c.x, c.y + hh + lift + 3,
      c.x - hw, c.y + lift + 3,
    ]).fill({ color: DERIVED.bridgeWoodDark, alpha: 0.35 });
    // Deck top.
    g.poly([
      c.x, c.y - hh + lift,
      c.x + hw, c.y + lift,
      c.x, c.y + hh + lift,
      c.x - hw, c.y + lift,
    ]).fill({ color: DERIVED.bridgeWood, alpha: 0.95 });
    // Plank seam strokes (2–3 lines).
    const seams = 2 + (rng.int(0, 1));
    for (let s = 1; s < seams; s++) {
      const t = s / seams;
      const sx = c.x + (t - 0.5) * hw * 0.7;
      g.moveTo(sx - 4, c.y - hh * 0.35 + lift)
        .lineTo(sx + 4, c.y + hh * 0.35 + lift)
        .stroke({ color: DERIVED.bridgeWoodDark, alpha: 0.45, width: 1 });
    }
  }

  // Mooring posts along the south (+gy in cart ≈ SE in iso) edge of the pier.
  for (let p = 0; p < plan.posts; p++) {
    const t = plan.posts === 1 ? 0.5 : p / (plan.posts - 1);
    const gx = plan.startGx + t * (plan.length - 1);
    const c = cartToIso(gx + 0.5, gy + 0.72);
    const ph = 7 + rng.range(0, 2);
    g.rect(c.x - 1.2, c.y - ph, 2.4, ph).fill({ color: DERIVED.bridgeWoodDark });
    g.ellipse(c.x, c.y - ph, 2.2, 1.4).fill({ color: DERIVED.bridgeWood });
  }

  // Small rowboat silhouette moored alongside mid-pier.
  const midGx = plan.startGx + (plan.length - 1) * 0.5;
  const boatGy = gy + plan.boatSide * 0.55;
  const bc = cartToIso(midGx + 0.5, boatGy + 0.5);
  const bw = 14;
  const bh = 5;
  // Hull shadow.
  g.ellipse(bc.x, bc.y + 2, bw * 0.9, bh * 0.55).fill({
    color: DERIVED.bridgeWoodDark,
    alpha: 0.3,
  });
  // Hull body (pointed canoe shape).
  g.poly([
    bc.x - bw, bc.y,
    bc.x - bw * 0.4, bc.y - bh,
    bc.x + bw * 0.4, bc.y - bh,
    bc.x + bw, bc.y,
    bc.x + bw * 0.4, bc.y + bh * 0.55,
    bc.x - bw * 0.4, bc.y + bh * 0.55,
  ]).fill({ color: DERIVED.bridgeWoodDark, alpha: 0.92 });
  // Gunwale highlight.
  g.poly([
    bc.x - bw * 0.7, bc.y - 0.5,
    bc.x - bw * 0.3, bc.y - bh * 0.75,
    bc.x + bw * 0.3, bc.y - bh * 0.75,
    bc.x + bw * 0.7, bc.y - 0.5,
  ]).fill({ color: DERIVED.bridgeWood, alpha: 0.7 });
  // Tiny thwart.
  g.rect(bc.x - 3, bc.y - 1.5, 6, 1.2).fill({
    color: DERIVED.bridgeWood,
    alpha: 0.8,
  });
}

/**
 * Draw all planned piers into one Graphics (caller parents + destroys).
 */
export function drawPiers(plans: readonly PierPlan[]): Graphics {
  const g = new Graphics();
  for (const p of plans) drawPier(g, p);
  return g;
}

// ---------------------------------------------------------------------------
// Draw — shoreline decoration
// ---------------------------------------------------------------------------

function drawShoreRock(g: Graphics, cx: number, cy: number, seed: number): void {
  const rng = rngFromCoords(seed, seed + 17);
  const rocks = rng.int(1, 2);
  for (let i = 0; i < rocks; i++) {
    const ox = rng.jitter(10);
    const oy = rng.jitter(5);
    const s = rng.range(2, 4.5);
    const rot = rng.range(0, Math.PI);
    const pts: number[] = [];
    for (let k = 0; k < 5; k++) {
      const a = rot + (k / 5) * Math.PI * 2;
      const rr = s * (0.7 + (k % 2) * 0.35);
      pts.push(cx + ox + Math.cos(a) * rr, cy + oy + Math.sin(a) * rr * 0.55);
    }
    g.poly(pts).fill({
      color: rng.bool(0.5) ? DERIVED.rock : DERIVED.rockDark,
      alpha: 0.9,
    });
  }
}

function drawShoreReed(g: Graphics, cx: number, cy: number, seed: number): void {
  const rng = rngFromCoords(seed, seed + 31);
  const tufts = rng.int(2, 3);
  for (let i = 0; i < tufts; i++) {
    const ox = rng.jitter(8);
    const baseY = cy + rng.jitter(3);
    const h = rng.range(5, 9);
    const lean = rng.jitter(2.5);
    g.moveTo(cx + ox, baseY)
      .lineTo(cx + ox + lean, baseY - h)
      .stroke({
        color: i % 2 === 0 ? DERIVED.olive : DERIVED.oliveDark,
        alpha: 0.85,
        width: 1.4,
      });
    // Tiny tip.
    g.circle(cx + ox + lean, baseY - h, 1.1).fill({
      color: DERIVED.oliveLight,
      alpha: 0.7,
    });
  }
}

function drawShoreFoam(g: Graphics, cx: number, cy: number, seed: number): void {
  const rng = rngFromCoords(seed, seed + 53);
  // Thin light arc on the waterline (static — water body already animates).
  const arcs = rng.int(1, 2);
  for (let i = 0; i < arcs; i++) {
    const ox = rng.jitter(6);
    const oy = rng.jitter(3);
    const r = rng.range(6, 11);
    // Approximate arc with a short polyline (two segments).
    g.moveTo(cx + ox - r, cy + oy)
      .lineTo(cx + ox - r * 0.3, cy + oy - r * 0.35)
      .lineTo(cx + ox + r * 0.5, cy + oy - r * 0.15)
      .stroke({ color: DERIVED.waterFoam, alpha: 0.35, width: 1.2 });
  }
}

/** Draw one shoreline prop at its iso centre. */
export function drawShoreDecorItem(g: Graphics, item: ShoreDecorItem): void {
  const c = cartToIso(item.gx + 0.5, item.gy + 0.5);
  const seed = hashCoords(item.gx, item.gy);
  if (item.kind === "rock") drawShoreRock(g, c.x, c.y, seed);
  else if (item.kind === "reed") drawShoreReed(g, c.x, c.y, seed);
  else drawShoreFoam(g, c.x, c.y, seed);
}

/**
 * Draw shoreline decoration chunked by tile grid (same CHUNK_SIZE as terrain).
 * Returns containers ready to parent into the terrain layer; caller tracks them
 * for viewport culling like water/sand chunks.
 */
export function drawShorelineDecor(
  items: readonly ShoreDecorItem[],
  chunkSize: number = 8,
): { key: string; container: Container }[] {
  if (items.length === 0) return [];
  const accs = new Map<string, Graphics>();
  for (const item of items) {
    const key = `${Math.floor(item.gx / chunkSize)},${Math.floor(item.gy / chunkSize)}`;
    let g = accs.get(key);
    if (!g) {
      g = new Graphics();
      accs.set(key, g);
    }
    drawShoreDecorItem(g, item);
  }
  const out: { key: string; container: Container }[] = [];
  for (const [key, g] of accs) {
    const container = new Container();
    container.addChild(g);
    out.push({ key, container });
  }
  return out;
}
