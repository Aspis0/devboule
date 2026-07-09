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

/** A raised stone arch bridge deck spanning one river tile (walkable).
 *  Replaces the earlier flat wooden plank: Caesar III stone bridge style with
 *  pier blocks, arch openings, paver deck with camber, parapets, and
 *  end-ramps. All geometry is static (zero per-frame cost) and deterministic.
 *  Orientation + per-side exposed-end flags are inferred once in
 *  buildTerrainFrame and passed in so the function stays self-contained.
 *
 *  Axis convention (orientation → geometry mapping):
 *    horizontal: run = screen-x (+48px per tile); perpendicular = screen-y
 *    vertical:   run = iso y (−48,+24 per tile); perpendicular = iso x
 *  Pier faces at tile boundaries use FULL HW/HH (not the deck-inset hw/hh)
 *  so adjacent tiles' piers share the boundary exactly. */
function drawBridgeDeck(
  g: Graphics,
  gx: number,
  gy: number,
  orientation: "horizontal" | "vertical",
  endBefore: boolean,
  endAfter: boolean,
): void {
  const LIFT = 7; // px the deck floats above the water surface
  const DECK_INSET = 0.82; // deck narrower than full tile (parapets on edges)
  const PIER_INSET = 0.12; // inner pier edge fraction (arch start)
  const PIER_H = 11; // pier block height (screen px)
  const PARAPET_H = 3; // parapet wall height above deck

  const c = cartToIso(gx + 0.5, gy + 0.5);
  const hw = HW * DECK_INSET; // deck half-width (inset from tile edge)
  const hh = HH * DECK_INSET; // deck half-height (inset from tile edge)
  const cLift = { x: c.x, y: c.y - LIFT };

  // ------------------------------------------------------------------
  // Orientation-dependent axis mapping.
  //
  // For HORIZONTAL: the bridge run is along screen-x (+48 px per grid step).
  //   dPerp is applied to screen-y for side walls / parapets.
  //   Pier faces at tile boundaries sit at c.x ± HW.
  //
  // For VERTICAL: the bridge run is along iso y (−48,+24 per grid step).
  //   dPerp is applied to screen-x for side walls / parapets.
  //   Pier faces at tile boundaries sit at c.y ± HH.
  // ------------------------------------------------------------------
  const isH = orientation === "horizontal";

  // Perpendicular offset for side walls / parapets.
  const dPerp = isH ? -HW * 0.08 : -HH * 0.08;

  // Pier face half-width (perpendicular to run, fraction of deck hw/hh).
  const pHW = 0.88;

  // ------------------------------------------------------------------
  // (a) Pier shadow — dark translucent ellipse on the water beneath
  // ------------------------------------------------------------------
  g.ellipse(c.x, c.y + 2, HW * 0.72, HH * 0.72).fill({
    color: DERIVED.bridgeStoneDark,
    alpha: 0.18,
  });

  // ------------------------------------------------------------------
  // (b) Side walls with arch openings — pier blocks at tile ends + arch
  // ------------------------------------------------------------------
  // Helper: draw one pier block quad at a position along the run axis
  // (runOffset) and perpendicular offset (pDepth) from the tile centre.
  const drawPierBlock = (
    runOffset: number, // +HW or −HW
    pDepth: number, // perpendicular offset from tile centre
  ): void => {
    if (isH) {
      // Horizontal: run axis = screen-x, perpendicular = screen-y.
      g.poly([
        c.x + runOffset - hw * pHW, c.y + pDepth,
        c.x + runOffset + hw * pHW, c.y + pDepth,
        c.x + runOffset + hw * pHW, c.y + pDepth - PIER_H,
        c.x + runOffset - hw * pHW, c.y + pDepth - PIER_H,
      ]).fill({ color: DERIVED.bridgeStone });
    } else {
      // Vertical: run axis = iso y, perpendicular = iso x.
      g.poly([
        c.x + pDepth, c.y + runOffset - hh * pHW,
        c.x + pDepth, c.y + runOffset + hh * pHW,
        c.x + pDepth, c.y + runOffset + hh * pHW - PIER_H,
        c.x + pDepth, c.y + runOffset - hh * pHW - PIER_H,
      ]).fill({ color: DERIVED.bridgeStone });
    }
  };

  // Helper: draw arch-opening polygon on one side wall.
  const drawArch = (
    pDepth: number,
    archRatio: number,
    archCrown: number,
  ): void => {
    const archH = PIER_H * archCrown;
    const innerR = PIER_INSET;
    const midFrac = innerR + archRatio * 0.12;
    if (isH) {
      g.poly([
        c.x - hw * innerR, c.y - hh * innerR + pDepth,
        c.x - hw * midFrac, c.y - hh * midFrac + pDepth - archH * 0.5,
        c.x, c.y + pDepth - archH,
        c.x + hw * midFrac, c.y + hh * midFrac + pDepth - archH * 0.5,
        c.x + hw * innerR, c.y + hh * innerR + pDepth,
      ]).fill({ color: DERIVED.bridgeStoneDark });
    } else {
      g.poly([
        c.x + pDepth, c.y - hh * innerR,
        c.x + pDepth - archH * 0.5, c.y - hh * midFrac,
        c.x - archH, c.y,
        c.x + pDepth - archH * 0.5, c.y + hh * midFrac,
        c.x + pDepth, c.y + hh * innerR,
      ]).fill({ color: DERIVED.bridgeStoneDark });
    }
  };

  // Draw two side walls (one per perpendicular side).
  for (const sgn of [-1, 1]) {
    const pDepth = sgn * dPerp;
    // Outer pier blocks at both tile ends — positioned at the tile boundary
    // (c.x ± HW for horizontal, c.y ± HH for vertical) so adjacent tiles'
    // piers overlap at the shared boundary.
    const runEnd = isH ? HW : HH;
    drawPierBlock(-runEnd, pDepth);
    drawPierBlock(+runEnd, pDepth);
    // Inner pier blocks (near centre, define arch opening edges).
    drawPierBlock(-runEnd * PIER_INSET, pDepth);
    drawPierBlock(+runEnd * PIER_INSET, pDepth);
    // Arch opening between inner piers.
    drawArch(pDepth, 0.76, 0.62);
  }

  // ------------------------------------------------------------------
  // (c) Deck — paver pattern
  // ------------------------------------------------------------------
  const top = [
    cLift.x,
    cLift.y - hh,
    cLift.x + hw,
    cLift.y,
    cLift.x,
    cLift.y + hh,
    cLift.x - hw,
    cLift.y,
  ];
  g.poly(top).fill({ color: DERIVED.bridgeStone, alpha: 1 });
  // Paver seam lines across the deck top (parallel to the bridge run).
  for (let i = 1; i < 4; i++) {
    const t = i / 4;
    if (isH) {
      // Horizontal: seams run along the NE–SW diagonal (parallel to run).
      const ax = cLift.x - hw + t * hw;
      const ay = cLift.y + t * hh;
      const bx = cLift.x + t * hw;
      const by = cLift.y - hh + t * hh;
      g.moveTo(ax, ay).lineTo(bx, by);
    } else {
      // Vertical: seams run along the NW–SE diagonal (parallel to run).
      const ax = cLift.x - hw + t * hw;
      const ay = cLift.y - t * hh;
      const bx = cLift.x - t * hw;
      const by = cLift.y + hh - t * hh;
      g.moveTo(ax, ay).lineTo(bx, by);
    }
  }
  g.stroke({ color: DERIVED.bridgeStoneDark, alpha: 0.45, width: 1 });

  // ------------------------------------------------------------------
  // (d) Parapets — low walls along both long sides with coping line
  // ------------------------------------------------------------------
  const drawParapet = (pDepth: number): void => {
    const pOuter = 0.96;
    const pInner = 0.86;
    const wall = [
      { x: cLift.x - hw * pOuter, y: cLift.y - hh * pOuter + pDepth },
      { x: cLift.x + hw * pOuter, y: cLift.y + hh * pOuter + pDepth },
      { x: cLift.x + hw * pInner, y: cLift.y + hh * pInner + pDepth - PARAPET_H },
      { x: cLift.x - hw * pInner, y: cLift.y - hh * pInner + pDepth - PARAPET_H },
    ];
    g.poly([
      wall[0].x, wall[0].y,
      wall[1].x, wall[1].y,
      wall[2].x, wall[2].y,
      wall[3].x, wall[3].y,
    ]).fill({ color: DERIVED.bridgeStoneDark });
    // Lighter coping line on top
    g.moveTo(wall[3].x, wall[3].y).lineTo(wall[2].x, wall[2].y);
    g.stroke({ color: DERIVED.bridgeStone, alpha: 0.85, width: 1.2 });
  };
  // Parapet offset matches the side wall perpendicular offset.
  const pOff = isH ? -HW * 0.1 : -HH * 0.1;
  drawParapet(pOff);
  drawParapet(-pOff);

  // ------------------------------------------------------------------
  // (e) End tiles: short ramp skirt + two small stone end-posts
  //     Only drawn on the EXPOSED side(s) (no live neighbour).
  // ------------------------------------------------------------------
  const rampOff = 0.14; // how far past the tile edge the ramp extends
  const postW = 3;
  const postH = 6;
  const postAlongFrac = 0.6; // end-post span along the perpendicular

  const drawEndTile = (side: "before" | "after"): void => {
    if (isH) {
      const sign = side === "before" ? -1 : 1;
      // Ramp: two triangle halves extending beyond the tile boundary along run.
      const runFrac = sign * (1 + rampOff);
      const rampA = [
        cLift.x + hw * runFrac, cLift.y + hh * runFrac,
        cLift.x, cLift.y + hh,
        cLift.x + hw * sign, cLift.y,
      ];
      const rampB = [
        cLift.x + hw * runFrac, cLift.y - hh * runFrac,
        cLift.x, cLift.y - hh,
        cLift.x + hw * sign, cLift.y,
      ];
      g.poly(rampA).fill({ color: DERIVED.bridgeStone, alpha: 0.7 });
      g.poly(rampB).fill({ color: DERIVED.bridgeStone, alpha: 0.7 });
      // End-posts: two stone pillars flanking the entrance on the deck surface.
      const postX = cLift.x + hw * sign * 0.92;
      const postY0 = cLift.y - hh * postAlongFrac;
      const postY1 = cLift.y + hh * postAlongFrac;
      g.rect(postX - postW / 2, postY0 - postH, postW, postH).fill({
        color: DERIVED.bridgeStoneDark,
      });
      g.rect(postX - postW / 2, postY1 - postH, postW, postH).fill({
        color: DERIVED.bridgeStoneDark,
      });
    } else {
      const sign = side === "before" ? -1 : 1;
      // Ramp: two triangle halves extending beyond the tile boundary.
      const runFrac = sign * (1 + rampOff);
      const rampA = [
        cLift.x - hw * runFrac, cLift.y + hh * runFrac,
        cLift.x - hw, cLift.y,
        cLift.x, cLift.y + hh * sign,
      ];
      const rampB = [
        cLift.x + hw * runFrac, cLift.y + hh * runFrac,
        cLift.x + hw, cLift.y,
        cLift.x, cLift.y + hh * sign,
      ];
      g.poly(rampA).fill({ color: DERIVED.bridgeStone, alpha: 0.7 });
      g.poly(rampB).fill({ color: DERIVED.bridgeStone, alpha: 0.7 });
      // End-posts: flanking the entrance on the deck surface.
      const postX0 = cLift.x - hw * postAlongFrac;
      const postX1 = cLift.x + hw * postAlongFrac;
      const postY = cLift.y + hh * sign * 0.92;
      g.rect(postX0 - postW / 2, postY - postH, postW, postH).fill({
        color: DERIVED.bridgeStoneDark,
      });
      g.rect(postX1 - postW / 2, postY - postH, postW, postH).fill({
        color: DERIVED.bridgeStoneDark,
      });
    }
  };

  if (endBefore) drawEndTile("before");
  if (endAfter) drawEndTile("after");
}
