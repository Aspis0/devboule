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
import { DERIVED, ALPHA } from "./palette";
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

// Hard cap on terrain tiles so a pathological extent can't explode the draw.
const MAX_TILES = 6000;

/** Push the 4 corner coords of tile (tx, ty)'s iso diamond into `out`. */
function tileDiamond(tx: number, ty: number): number[] {
  // Center of the tile in iso space.
  const c = cartToIso(tx, ty);
  return [
    c.x,
    c.y - HH, // top
    c.x + HW,
    c.y, // right
    c.x,
    c.y + HH, // bottom
    c.x - HW,
    c.y, // left
  ];
}

/**
 * Draw the mottled ground into a set of Graphics added to `layer`.
 * Returns the Graphics so the caller owns destruction.
 *
 * We batch by shade: one Graphics accumulates all tiles of a given band, which
 * keeps the GPU geometry compact (a handful of fills) regardless of tile count.
 */
export function drawTerrain(
  ext: TerrainExtent,
): { graphics: Graphics[]; tileCount: number } {
  const bands: { color: number; g: Graphics }[] = [
    { color: DERIVED.groundDark, g: new Graphics() },
    { color: DERIVED.groundMid, g: new Graphics() },
    { color: DERIVED.groundLight, g: new Graphics() },
  ];
  const dirtG = new Graphics();
  const seamG = new Graphics();

  // Single pass over the tile grid: pick the mottling band AND roll the sparse
  // dirt patch per tile. Both are seeded purely by (tileX, tileY) via valueNoise
  // (no per-tile Rng allocation), so the ground stays deterministic — a re-scan
  // reproduces the same decoration. Three independent value-noise samples per
  // tile (offset coords) drive the patch roll / size / worn flavour.
  let count = 0;
  for (let ty = ext.minY; ty <= ext.maxY && count < MAX_TILES; ty++) {
    for (let tx = ext.minX; tx <= ext.maxX && count < MAX_TILES; tx++) {
      count++;
      // Two-octave value noise: a low-frequency field (samples on a /3 grid so
      // neighbours correlate into broad patches) plus a per-tile detail sample.
      // The blend gives clumped grass/earth regions with crisp per-tile breakup
      // — Zeus-style mottling instead of uniform mud. Still fully deterministic.
      const lo = valueNoise(Math.floor(tx / 3), Math.floor(ty / 3));
      const hi = valueNoise(tx, ty);
      const n = lo * 0.62 + hi * 0.38;
      // Three flat bands with a wider, more even split so the light/dark grass
      // patches actually read as distinct (was 0.28/0.78 → mostly one band).
      const band = n < 0.36 ? 0 : n < 0.68 ? 1 : 2;
      const poly = tileDiamond(tx, ty);
      bands[band].g.poly(poly).fill({ color: bands[band].color, alpha: 1 });

      // Worn dirt / sand patch — now ~13% (was ~6%) so bare-earth patches give
      // the green a warm counterpoint, drawn as a slightly inset diamond so the
      // base band still frames it. Offset coords give decorrelated rolls.
      const rRoll = valueNoise(tx ^ 0x5bd1, ty ^ 0x9e37);
      if (rRoll >= 0.13) continue;
      const c = cartToIso(tx, ty);
      const s = 0.45 + valueNoise(tx ^ 0x1234, ty ^ 0xabcd) * (0.8 - 0.45);
      const worn = valueNoise(tx ^ 0x7777, ty ^ 0x3333) < 0.4;
      dirtG
        .poly([
          c.x,
          c.y - HH * s,
          c.x + HW * s,
          c.y,
          c.x,
          c.y + HH * s,
          c.x - HW * s,
          c.y,
        ])
        .fill({
          color: worn ? DERIVED.groundWorn : DERIVED.groundDirt,
          alpha: 0.7,
        });
    }
  }

  // Subtle iso tile seams: only the NE/NW edges of each tile, faint, every
  // other tile, so the grid is felt rather than seen. Cheap single stroke.
  for (let ty = ext.minY; ty <= ext.maxY; ty += 1) {
    const a = cartToIso(ext.minX, ty);
    const b = cartToIso(ext.maxX, ty);
    seamG.moveTo(a.x, a.y).lineTo(b.x, b.y);
  }
  for (let tx = ext.minX; tx <= ext.maxX; tx += 1) {
    const a = cartToIso(tx, ext.minY);
    const b = cartToIso(tx, ext.maxY);
    seamG.moveTo(a.x, a.y).lineTo(b.x, b.y);
  }
  seamG.stroke({ color: DERIVED.seam, alpha: ALPHA.seam, width: 1 });

  return {
    graphics: [bands[0].g, bands[1].g, bands[2].g, dirtG, seamG],
    tileCount: count,
  };
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
//   - raised wooden bridge decks over the river tiles a road crosses.
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

  // 3) Bridge decks — a raised wooden plank over the crossed river tile. Drawn
  //    last so they sit visually on top of the water. Sorted back→front (depth)
  //    so overlapping decks layer correctly.
  const bridges = [...terrain.bridges].sort(
    (p, q) => p.gx + p.gy - (q.gx + q.gy),
  );
  for (const b of bridges) {
    const a = accOf(b.gx, b.gy);
    drawBridgeDeck(a.bridges, b.gx, b.gy);
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

/** A raised wooden bridge deck spanning one river tile (walkable). Ported from
 *  `drawBridge` in `js/map_app.js`: a slightly inset wooden quad lifted off the
 *  water with plank seams + corner rail posts. Static geometry (no per-frame
 *  cost). The river still flows visibly under the inset edges. */
function drawBridgeDeck(g: Graphics, gx: number, gy: number): void {
  // Raised deck: the tile diamond lifted by a small screen-space z so it reads
  // as a deck above the water. We lift by drawing the diamond shifted up.
  const LIFT = 7; // px the deck floats above the water surface
  const c = cartToIso(gx + 0.5, gy + 0.5);
  const inset = 0.92; // deck is slightly smaller than the tile (water peeks at edges)
  const hw = HW * inset;
  const hh = HH * inset;
  const top = [
    c.x,
    c.y - hh - LIFT,
    c.x + hw,
    c.y - LIFT,
    c.x,
    c.y + hh - LIFT,
    c.x - hw,
    c.y - LIFT,
  ];
  // Deck side (the dark under-board) — a thin skirt between the lifted top and
  // the water, drawn first so the top sits on it.
  g.poly([
    c.x - hw,
    c.y - LIFT,
    c.x,
    c.y + hh - LIFT,
    c.x,
    c.y + hh,
    c.x - hw,
    c.y,
  ]).fill({ color: DERIVED.bridgeWoodDark, alpha: 1 });
  g.poly([
    c.x,
    c.y + hh - LIFT,
    c.x + hw,
    c.y - LIFT,
    c.x + hw,
    c.y,
    c.x,
    c.y + hh,
  ]).fill({ color: DERIVED.bridgeWoodDark, alpha: 1 });
  // Deck top.
  g.poly(top).fill({ color: DERIVED.bridgeWood, alpha: 1 });
  // Plank seams across the deck top (cheap static strokes).
  for (let i = 1; i < 4; i++) {
    const tt = i / 4;
    // Interpolate left→right across the top diamond at parameter tt (front face).
    const ax = c.x - hw + tt * hw;
    const ay = c.y - LIFT + tt * hh;
    const bx = c.x + tt * hw;
    const by = c.y - hh - LIFT + tt * hh;
    g.moveTo(ax, ay).lineTo(bx, by);
  }
  g.stroke({ color: DERIVED.bridgeWoodDark, alpha: 0.5, width: 1 });
}
