// Terrain — chunky, deterministic, retro tile-art ground.
//
// Replaces the single flat grass diamond with PER-TILE value-noise tinting: for
// every integer tile in the populated bbox (+ margin) we pick a flat ground
// shade from a deterministic hash of (tileX, tileY), so the ground reads
// mottled and asymmetric like Caesar III / Pharaoh tile art — no smoothing,
// each tile is a single flat value (that IS the retro look). On top we add
// occasional worn dirt patches and subtle iso tile seams.
//
// DETERMINISM: tint + patches are seeded purely by (tileX, tileY) via the rng
// helpers, so a re-scan reproduces the identical ground. The terrain is pure
// DECORATION — it never asserts a building exists on a tile.
//
// PERFORMANCE: everything is baked ONCE into a small fixed number of Graphics
// (one per shade band + one for seams) at setCityState time. Nothing here is
// touched per frame.

import { Container, Graphics } from "pixi.js";
import { cartToIso } from "./iso";
import { DERIVED } from "./palette";
import { valueNoise } from "./rng";
import type { TerrainData } from "../../types/city";

export interface TerrainExtent {
  minX: number;
  minY: number;
  maxX: number;
  maxY: number;
}

/** Compute the integer tile bbox covering the buildings, expanded by margin. */
export function computeExtent(
  coords: { x: number; y: number }[],
  fallbackW: number,
  fallbackH: number,
  margin: number,
): TerrainExtent {
  if (coords.length === 0) {
    return {
      minX: -margin,
      minY: -margin,
      maxX: Math.max(fallbackW, 8) + margin,
      maxY: Math.max(fallbackH, 8) + margin,
    };
  }
  let minX = Infinity,
    minY = Infinity,
    maxX = -Infinity,
    maxY = -Infinity;
  for (const c of coords) {
    minX = Math.min(minX, c.x);
    minY = Math.min(minY, c.y);
    maxX = Math.max(maxX, c.x);
    maxY = Math.max(maxY, c.y);
  }
  return {
    minX: Math.floor(minX) - margin,
    minY: Math.floor(minY) - margin,
    maxX: Math.ceil(maxX) + margin,
    maxY: Math.ceil(maxY) + margin,
  };
}

// Half tile in iso space (TILE_W=96, TILE_H=48 -> 48 / 24).
const HW = 48;
const HH = 24;

// Hard cap on ACCENT patches (meadow tone variation on top of the full-extent
// base fill) so a pathological extent can't explode the draw. The base ground
// itself is a single 4-vertex polygon, so coverage is always 100% regardless
// of this cap — the cap only bounds decoration density.
const MAX_ACCENTS = 1600;

// Accent sampling lattice step (tiles). Coarser than 1 so accents read as
// meadow patches, not per-tile noise.
const ACCENT_STEP = 2;

// The lattice is visited in PHASES interleaved passes (i, i+PHASES, ...) so a
// cap hit truncates density uniformly across the WHOLE map instead of filling
// the north corner row-major and leaving the south bare.
const PHASES = 16;

// Max fills per Graphics chunk. Pixi v8 marks Graphics with ≥400 vertices as
// non-batchable (each shape primitive → a separate GL draw call). A 4-point
// polygon fill = 4 vertices, so 80 fills = 320 vertices, safely under the
// threshold. This keeps the terrain layer batchable → O(10) draw calls instead
// of O(7000).
const CHUNK_FILLS = 80;

// Hard cap on dirt/sand patches so a pathological extent can't explode the draw.
const MAX_DIRT = 500;


/**
 * Draw the ground into a FLAT array of Graphics.
 * Returns the Graphics (caller owns destruction) + painted shape count.
 *
 * ARCHITECTURE (T6b, replaces the per-tile fill approach):
 *  1. BASE — ONE 4-vertex polygon covering the entire extent, painted
 *     groundMid. Coverage is 100% at O(1) cost no matter how large the map
 *     is (the old per-tile loop capped at 6,000 of ~69,000 tiles, leaving
 *     91% of the map as raw page background — the "empty white map" bug).
 *  2. ACCENTS — bounded meadow tone patches (dark/light) + dirt patches on
 *     top, sampled on a coarse lattice visited in PHASES interleaved passes
 *     so cap hits thin density uniformly instead of clustering north.
 *
 * PERFORMANCE (T6a rules kept): fills chunked at ≤CHUNK_FILLS per Graphics
 * so everything stays batchable. No tile grid is drawn at all — Caesar III
 * ground has no grid, and the old full-extent line pass was both ugly and
 * the main unbatchable GPU load.
 */
export function drawTerrain(
  ext: TerrainExtent,
): { graphics: Graphics[]; gridGraphics: Graphics | null; tileCount: number } {
  const out: Graphics[] = [];

  // --- 1. Full-coverage base: the extent rectangle projected to iso. Tile
  // (t) is centred on cartToIso(t), so the rect spans ±0.5 beyond the ends.
  const a = cartToIso(ext.minX - 0.5, ext.minY - 0.5);
  const b = cartToIso(ext.maxX + 0.5, ext.minY - 0.5);
  const c = cartToIso(ext.maxX + 0.5, ext.maxY + 0.5);
  const d = cartToIso(ext.minX - 0.5, ext.maxY + 0.5);
  const base = new Graphics();
  base.poly([a.x, a.y, b.x, b.y, c.x, c.y, d.x, d.y]).fill({
    color: DERIVED.groundMid,
    alpha: 1,
  });
  out.push(base);

  // --- 2. Accent + dirt patches on an interleaved coarse lattice.
  const cols = Math.max(1, Math.floor((ext.maxX - ext.minX + 1) / ACCENT_STEP));
  const rows = Math.max(1, Math.floor((ext.maxY - ext.minY + 1) / ACCENT_STEP));
  const latticeN = cols * rows;

  let accentG = new Graphics();
  let accentFills = 0;
  let accentTotal = 0;
  let dirtG = new Graphics();
  let dirtFills = 0;
  let dirtTotal = 0;
  let count = 1; // the base polygon

  for (
    let phase = 0;
    phase < PHASES && (accentTotal < MAX_ACCENTS || dirtTotal < MAX_DIRT);
    phase++
  ) {
    for (
      let i = phase;
      i < latticeN && (accentTotal < MAX_ACCENTS || dirtTotal < MAX_DIRT);
      i += PHASES
    ) {
      const tx = ext.minX + (i % cols) * ACCENT_STEP;
      const ty = ext.minY + Math.floor(i / cols) * ACCENT_STEP;

      // Two-octave value noise picks the meadow tone band.
      const lo = valueNoise(Math.floor(tx / 3), Math.floor(ty / 3));
      const hi = valueNoise(tx, ty);
      const n = lo * 0.62 + hi * 0.38;

      // Meadow tone patch (dark or light band only — mid IS the base).
      if ((n < 0.36 || n > 0.68) && accentTotal < MAX_ACCENTS) {
        accentTotal++;
        const cc = cartToIso(tx, ty);
        // Patch radius 1.2..2.4 tiles — reads as a meadow, not tile noise.
        const s = 1.2 + valueNoise(tx ^ 0x51ed, ty ^ 0x2b9c) * 1.2;
        if (accentFills >= CHUNK_FILLS) {
          out.push(accentG);
          accentG = new Graphics();
          accentFills = 0;
        }
        accentG
          .poly([
            cc.x, cc.y - HH * s,
            cc.x + HW * s, cc.y,
            cc.x, cc.y + HH * s,
            cc.x - HW * s, cc.y,
          ])
          .fill({
            color: n < 0.36 ? DERIVED.groundDark : DERIVED.groundLight,
            alpha: 0.55,
          });
        accentFills++;
        count++;
      }

      // Dirt patch — sparse, warm contrast on the green.
      const rRoll = valueNoise(tx ^ 0x5bd1, ty ^ 0x9e37);
      if (rRoll < 0.1 && dirtTotal < MAX_DIRT) {
        dirtTotal++;
        const cc = cartToIso(tx, ty);
        const s = 0.45 + valueNoise(tx ^ 0x1234, ty ^ 0xabcd) * (0.8 - 0.45);
        const worn = valueNoise(tx ^ 0x7777, ty ^ 0x3333) < 0.4;
        if (dirtFills >= CHUNK_FILLS) {
          out.push(dirtG);
          dirtG = new Graphics();
          dirtFills = 0;
        }
        dirtG
          .poly([
            cc.x, cc.y - HH * s,
            cc.x + HW * s, cc.y,
            cc.x, cc.y + HH * s,
            cc.x - HW * s, cc.y,
          ])
          .fill({
            color: worn ? DERIVED.groundWorn : DERIVED.groundDirt,
            alpha: 0.7,
          });
        dirtFills++;
        count++;
      }
    }
  }
  if (accentFills > 0) out.push(accentG);
  if (dirtFills > 0) out.push(dirtG);

  // No tile grid: Caesar III ground has none, and the full-extent line pass
  // was the dominant unbatchable GPU cost. Callers already handle null.
  return { graphics: out, gridGraphics: null, tileCount: count };
}

// ===========================================================================
// WATER TERRAIN — sea + rivers + shores + bridges (Polis terrain frame).
//
// The backend (`terrain::build_terrain`) sends a SPARSE `TerrainData`: only the
// non-grass tiles (water/sand/bridges) + the river ranges + the sea edge. The
// grass land keeps its value-noise ground above. This renders the frame:
//   - sand shore tiles (flat diamonds, drawn UNDER water edges so beaches frame
//     the coast),
//   - sea + river water tiles (flat diamonds, blue, with a cheap animated
//     shimmer overlay that is ticked ONLY for visible chunks),
//   - raised stone arch bridge decks over the river tiles a road crosses.
//
// PERFORMANCE: tiles are bucketed into CHUNK-keyed Graphics so the renderer can
// cull whole off-screen chunks (the big-map win — water geometry is built ONCE,
// never per frame; only the visible chunks' shimmer is animated). Ported from
// `js/map_app.js` drawTile/makeWater/drawBridge math, adapted to this iso kit.
// ===========================================================================

/** Half-tile diamond corner offsets (same TILE_W=96/TILE_H=48 as the ground). */
function diamondAt(gx: number, gy: number): number[] {
  const c = cartToIso(gx + 0.5, gy + 0.5); // tile CENTER (backend tiles are cell-origin)
  return [c.x, c.y - HH, c.x + HW, c.y, c.x, c.y + HH, c.x - HW, c.y];
}

/** One animated water chunk: a base of flat blue diamonds + a shimmer overlay
 *  redrawn cheaply when ticked. `update` is a no-op cost unless the chunk is
 *  visible (the renderer only calls it for visible chunks). */
export interface WaterChunkAnim {
  /** Redraw the shimmer lines for time `t` (seconds). Cheap, alloc-bounded. */
  update(t: number): void;
}

/** A built terrain-frame chunk: its container (parent into the terrain layer)
 *  keyed by `chunkKey`, plus an optional shimmer anim ticked when visible. */
export interface TerrainChunk {
  key: string;
  container: Container;
  anim: WaterChunkAnim | null;
  /** iso bbox of the chunk's water for the cheap shimmer (screen space). */
}

/** Hard cap on water tiles drawn so a pathological extent can't explode the GPU.
 *  Exported so the warn-on-truncation behaviour is regression-testable against the
 *  exact cap (no magic-number drift between code and test). */
export const MAX_WATER_TILES = 40000;

/**
 * Build the water/sand/bridge terrain frame from a sparse `TerrainData`, bucketed
 * into chunks of `chunkSize` tiles (matching the renderer's building chunks so
 * culling lines up). Returns one {@link TerrainChunk} per non-empty chunk; the
 * caller parents each `container` into the terrain layer and toggles
 * `container.visible` from the cull pass, ticking `anim` only for visible chunks.
 *
 * Pure w.r.t. PixiJS construction (no app/ticker) so it is unit-testable for the
 * bucketing/teardown contract.
 */
export function buildTerrainFrame(
  terrain: TerrainData | undefined,
  chunkSize: number,
): TerrainChunk[] {
  if (!terrain) return [];
  const step = Math.max(1, Math.floor(chunkSize));
  const chunkKey = (gx: number, gy: number) =>
    `${Math.floor(gx / step)},${Math.floor(gy / step)}`;

  // Per-chunk accumulators. Water/sand are flat diamonds batched into a single
  // Graphics each; bridges are a separate Graphics drawn last (on top of water).
  interface Acc {
    sand: Graphics;
    water: Graphics;
    bridges: Graphics;
    // iso bbox of this chunk's water for the shimmer overlay.
    minX: number;
    maxX: number;
    minY: number;
    maxY: number;
    hasWater: boolean;
  }
  const accs = new Map<string, Acc>();
  const accOf = (gx: number, gy: number): Acc => {
    const key = chunkKey(gx, gy);
    let a = accs.get(key);
    if (!a) {
      a = {
        sand: new Graphics(),
        water: new Graphics(),
        bridges: new Graphics(),
        minX: Infinity,
        maxX: -Infinity,
        minY: Infinity,
        maxY: -Infinity,
        hasWater: false,
      };
      accs.set(key, a);
    }
    return a;
  };

  // 1) Sand shores first (so a water tile's diamond can overlap the beach edge).
  for (const s of terrain.sand) {
    const a = accOf(s.gx, s.gy);
    const poly = diamondAt(s.gx, s.gy);
    a.sand.poly(poly).fill({ color: DERIVED.shoreSand, alpha: 1 });
  }

  // 2) Water (sea + river). Deep open-sea uses the darker shade. Track the iso
  //    bbox per chunk for the shimmer overlay. Bounded by MAX_WATER_TILES — a
  //    pathological extent can't explode the GPU. If we DO hit the cap we warn
  //    ONCE (honest: names the cap + the real counts), because a silent break
  //    leaves a half-drawn sea with sand/bridges floating over bare ground.
  let drawn = 0;
  for (const w of terrain.water) {
    if (drawn >= MAX_WATER_TILES) {
      console.warn(
        `Polis terrain: water tile cap reached — drawing ${MAX_WATER_TILES} of ` +
          `${terrain.water.length} water tiles; the sea is truncated (sand/bridges ` +
          `beyond the cap may sit over bare ground).`,
      );
      break;
    }
    drawn++;
    const a = accOf(w.gx, w.gy);
    const poly = diamondAt(w.gx, w.gy);
    a.water.poly(poly).fill({
      color: w.deep ? DERIVED.waterDeep : DERIVED.waterMid,
      alpha: 1,
    });
    a.hasWater = true;
    // Track bbox (poly is [x,y, x,y, ...]).
    for (let i = 0; i < poly.length; i += 2) {
      a.minX = Math.min(a.minX, poly[i]);
      a.maxX = Math.max(a.maxX, poly[i]);
      a.minY = Math.min(a.minY, poly[i + 1]);
      a.maxY = Math.max(a.maxY, poly[i + 1]);
    }
  }

  // 3) Bridge decks — raised stone arch bridges over river tiles. Drawn last so
  //    they sit visually on top of the water. Sorted back→front (depth) so
  //    overlapping bridges layer correctly.
  //
  // Orientation inference: build an adjacency map from the sorted bridge list,
  // then determine each tile's orientation (horizontal/vertical) and per-side
  // exposed-end flags by looking for neighbours sharing an axis.
  // Build adjacency map: key "gx,gy" → true for each bridge tile.
  const bridgeSet = new Set<string>();
  for (const b of terrain.bridges) bridgeSet.add(`${b.gx},${b.gy}`);

  const bridges = [...terrain.bridges].sort(
    (p, q) => p.gx + p.gy - (q.gx + q.gy),
  );
  for (const b of bridges) {
    const a = accOf(b.gx, b.gy);
    // Determine orientation from neighbours. "horizontal" means the bridge
    // run follows the x-axis (dx=±1); "vertical" follows the y-axis (dy=±1).
    const hasH = bridgeSet.has(`${b.gx - 1},${b.gy}`) || bridgeSet.has(`${b.gx + 1},${b.gy}`);
    const hasV = bridgeSet.has(`${b.gx},${b.gy - 1}`) || bridgeSet.has(`${b.gx},${b.gy + 1}`);
    // MAJOR 1 fix: lone tile (hasH=false, hasV=false) → fallback "horizontal".
    // Only commit to "vertical" when the tile is UNAMBIGUOUSLY part of a
    // vertical run (hasV && !hasH). All other cases → "horizontal".
    const orientation: "horizontal" | "vertical" =
      hasV && !hasH ? "vertical" : "horizontal";
    // Per-side exposed end detection: before = negative neighbour missing,
    // after = positive neighbour missing, along the run axis.
    const endBefore = orientation === "horizontal"
      ? !bridgeSet.has(`${b.gx - 1},${b.gy}`)
      : !bridgeSet.has(`${b.gx},${b.gy - 1}`);
    const endAfter = orientation === "horizontal"
      ? !bridgeSet.has(`${b.gx + 1},${b.gy}`)
      : !bridgeSet.has(`${b.gx},${b.gy + 1}`);
    drawBridgeDeck(a.bridges, b.gx, b.gy, orientation, endBefore, endAfter);
  }

  // Assemble one container per chunk (sand → water → shimmer → bridges).
  const out: TerrainChunk[] = [];
  for (const [key, a] of accs) {
    const container = new Container();
    container.addChild(a.sand);
    container.addChild(a.water);

    let anim: WaterChunkAnim | null = null;
    if (a.hasWater && Number.isFinite(a.minX)) {
      const shimmer = new Graphics();
      container.addChild(shimmer);
      anim = makeShimmer(shimmer, a.minX, a.maxX, a.minY, a.maxY, key);
    }
    container.addChild(a.bridges);
    out.push({ key, container, anim });
  }
  return out;
}

/**
 * Cheap animated water shimmer over an iso bbox: a handful of horizontal wave
 * lines whose vertical offset oscillates with `t`. Ported from `makeWater` in
 * `js/map_app.js` but WITHOUT a per-tile mask (the lines are clipped implicitly
 * to the water by being faint and short-lived) so it stays allocation-free per
 * frame — `g.clear()` + a bounded number of `lineTo`s. Deterministic phase from
 * the chunk key so two chunks don't shimmer in lockstep.
 */
function makeShimmer(
  g: Graphics,
  minX: number,
  maxX: number,
  minY: number,
  maxY: number,
  key: string,
): WaterChunkAnim {
  // Deterministic per-chunk phase offset (no Math.random — stable across builds).
  let phase = 0;
  for (let i = 0; i < key.length; i++) phase = (phase * 31 + key.charCodeAt(i)) % 1000;
  const phase0 = (phase / 1000) * Math.PI * 2;
  const rows = Math.max(2, Math.min(12, Math.round((maxY - minY) / 18)));
  const stepX = Math.max(14, (maxX - minX) / 10);

  return {
    update(t: number): void {
      g.clear();
      for (let r = 0; r < rows; r++) {
        const yy = minY + (r / rows) * (maxY - minY);
        const off = Math.sin(t * 1.8 + r * 0.8 + phase0) * 4;
        g.moveTo(minX, yy + off);
        for (let x = minX; x <= maxX; x += stepX) {
          g.lineTo(x, yy + off + Math.sin(t * 2.4 + x * 0.05 + phase0) * 2);
        }
      }
      g.stroke({ color: DERIVED.waterFoam, alpha: 0.32, width: 1.2 });
    },
  };
}

/** A raised stone bridge deck spanning one river tile (walkable).
 *
 *  ISO-CORRECT GEOMETRY (T6d): every horizontal surface is projected from the
 *  tile's four cart-space corners via cartToIso, exactly like buildings and
 *  roads — so a bridge run reads correctly at EITHER orientation. (The old
 *  code mixed screen-axis rectangles with iso directions; since both grid
 *  axes are diagonal on screen, its stone blocks stuck out at wrong angles.)
 *  Vertical faces (walls, parapets, posts) drop straight down in screen
 *  space, which IS correct for vertical surfaces in an iso projection.
 *
 *  Corner naming (screen position): A=cartToIso(gx,gy) top, B=(gx+1,gy)
 *  right, C=(gx+1,gy+1) bottom, D=(gx,gy+1) left. A horizontal run (along
 *  grid x) connects through edges A–D / B–C and gets parapets on A–B / D–C;
 *  a vertical run swaps the two pairs. Camera-facing wall faces are B–C
 *  (south-east) and D–C (south-west). Adjacent bridge tiles share corner
 *  projections, so multi-tile spans join seamlessly.
 *
 *  All geometry is static (zero per-frame cost) and deterministic.
 *  Orientation + per-side exposed-end flags are inferred once in
 *  buildTerrainFrame and passed in so the function stays self-contained. */
function drawBridgeDeck(
  g: Graphics,
  gx: number,
  gy: number,
  orientation: "horizontal" | "vertical",
  endBefore: boolean,
  endAfter: boolean,
): void {
  const LIFT = 6; // px the deck floats above the water surface
  const WALL = 9; // wall depth from deck edge down toward the water
  const PARAPET_H = 3.5; // parapet wall height above the deck
  const POST_W = 3;
  const POST_H = 6;

  const isH = orientation === "horizontal";
  const c = cartToIso(gx + 0.5, gy + 0.5);

  // Tile corner projections (cart corners -> iso screen points).
  const A = cartToIso(gx, gy); // top
  const B = cartToIso(gx + 1, gy); // right
  const C = cartToIso(gx + 1, gy + 1); // bottom
  const D = cartToIso(gx, gy + 1); // left

  const lerp = (
    p: { x: number; y: number },
    q: { x: number; y: number },
    t: number,
  ): { x: number; y: number } => ({
    x: p.x + (q.x - p.x) * t,
    y: p.y + (q.y - p.y) * t,
  });

  // ------------------------------------------------------------------
  // (a) Shadow on the water beneath the span.
  // ------------------------------------------------------------------
  g.ellipse(c.x, c.y + 2, HW * 0.7, HH * 0.7).fill({
    color: DERIVED.bridgeStoneDark,
    alpha: 0.18,
  });

  // ------------------------------------------------------------------
  // (b) Front walls — the two camera-facing vertical faces, dropping from
  //     the lifted deck edge down toward the water. Two-tone for depth.
  // ------------------------------------------------------------------
  const wallQuad = (
    p1: { x: number; y: number },
    p2: { x: number; y: number },
    color: number,
  ): void => {
    g.poly([
      p1.x, p1.y - LIFT,
      p2.x, p2.y - LIFT,
      p2.x, p2.y - LIFT + WALL,
      p1.x, p1.y - LIFT + WALL,
    ]).fill({ color });
  };
  wallQuad(B, C, DERIVED.bridgeStone); // south-east face (lit)
  wallQuad(D, C, DERIVED.bridgeStoneDark); // south-west face (shaded)

  // Arch opening: a dark half-ellipse on the camera-facing face that is
  // PARALLEL to the run (the water passes under it). Horizontal run ->
  // D-C face; vertical run -> B-C face. The ellipse is centred on the
  // wall's bottom edge so its visible upper half reads as the opening
  // and the lower half blends into the water as a soft reflection.
  const archEdge: [typeof A, typeof A] = isH ? [D, C] : [B, C];
  const archMid = lerp(archEdge[0], archEdge[1], 0.5);
  const archHalfLen =
    Math.hypot(archEdge[1].x - archEdge[0].x, archEdge[1].y - archEdge[0].y) *
    0.26;
  g.ellipse(archMid.x, archMid.y - LIFT + WALL, archHalfLen, WALL * 0.72).fill({
    color: DERIVED.waterDeep,
    alpha: 0.85,
  });

  // ------------------------------------------------------------------
  // (c) Deck — the lifted tile diamond with paver seams along the run.
  // ------------------------------------------------------------------
  g.poly([
    A.x, A.y - LIFT,
    B.x, B.y - LIFT,
    C.x, C.y - LIFT,
    D.x, D.y - LIFT,
  ]).fill({ color: DERIVED.bridgeStone, alpha: 1 });
  // Camber highlight: a lighter band along the middle of the run.
  const bandLo = 0.32;
  const bandHi = 0.68;
  const bandCorners = isH
    ? [lerp(A, D, bandLo), lerp(B, C, bandLo), lerp(B, C, bandHi), lerp(A, D, bandHi)]
    : [lerp(A, B, bandLo), lerp(D, C, bandLo), lerp(D, C, bandHi), lerp(A, B, bandHi)];
  g.poly([
    bandCorners[0].x, bandCorners[0].y - LIFT,
    bandCorners[1].x, bandCorners[1].y - LIFT,
    bandCorners[2].x, bandCorners[2].y - LIFT,
    bandCorners[3].x, bandCorners[3].y - LIFT,
  ]).fill({ color: DERIVED.bridgeStoneLight, alpha: 0.35 });
  // Paver seams parallel to the run: from the "before" open edge to the
  // "after" open edge at fixed perpendicular fractions.
  for (const t of [0.25, 0.5, 0.75]) {
    const s = isH ? lerp(A, D, t) : lerp(A, B, t);
    const e = isH ? lerp(B, C, t) : lerp(D, C, t);
    g.moveTo(s.x, s.y - LIFT).lineTo(e.x, e.y - LIFT);
  }
  g.stroke({ color: DERIVED.bridgeStoneDark, alpha: 0.4, width: 1 });

  // ------------------------------------------------------------------
  // (d) Parapets — low raised walls on the two edges parallel to the run.
  // ------------------------------------------------------------------
  const parapetEdges: Array<[typeof A, typeof A]> = isH
    ? [[A, B], [D, C]]
    : [[A, D], [B, C]];
  for (const [p1, p2] of parapetEdges) {
    g.poly([
      p1.x, p1.y - LIFT,
      p2.x, p2.y - LIFT,
      p2.x, p2.y - LIFT - PARAPET_H,
      p1.x, p1.y - LIFT - PARAPET_H,
    ]).fill({ color: DERIVED.bridgeStoneDark });
    // Lighter coping line on top of the parapet.
    g.moveTo(p1.x, p1.y - LIFT - PARAPET_H).lineTo(p2.x, p2.y - LIFT - PARAPET_H);
    g.stroke({ color: DERIVED.bridgeStone, alpha: 0.85, width: 1.2 });
  }

  // ------------------------------------------------------------------
  // (e) End posts — two small stone pillars flanking each EXPOSED end
  //     (no live bridge neighbour on that side).
  // ------------------------------------------------------------------
  const drawEndPosts = (edge: [typeof A, typeof A]): void => {
    for (const t of [0.12, 0.88]) {
      const p = lerp(edge[0], edge[1], t);
      g.rect(p.x - POST_W / 2, p.y - LIFT - POST_H, POST_W, POST_H).fill({
        color: DERIVED.bridgeStoneDark,
      });
      // Lit cap on the post.
      g.rect(p.x - POST_W / 2, p.y - LIFT - POST_H, POST_W, 1.4).fill({
        color: DERIVED.bridgeStoneLight,
        alpha: 0.9,
      });
    }
  };
  // "before" = negative-neighbour edge along the run; "after" = positive.
  const beforeEdge: [typeof A, typeof A] = isH ? [A, D] : [A, B];
  const afterEdge: [typeof A, typeof A] = isH ? [B, C] : [D, C];
  if (endBefore) drawEndPosts(beforeEdge);
  if (endAfter) drawEndPosts(afterEdge);
}
