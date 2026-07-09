// Props — deterministic decorative scatter on EMPTY ground tiles.
//
// Like Caesar III's flora / rocks, props break the emptiness between buildings.
// They are pure DECORATION and must obey two rules:
//   1. HONESTY: a prop NEVER implies a file exists. We only ever scatter onto
//      tiles that are NOT occupied by a building, and props are never clickable
//      / never resolve to data.
//   2. DETERMINISM: placement, kind, scale, rotation and offset are all seeded
//      by (tileX, tileY) via rngFromCoords — a re-scan reproduces the identical
//      decoration. No Math.random().
//
// Kinds: olive micro-vegetation (clustered olive-green blobs), rocky debris
// (angular gray polygons), and the occasional small courtyard / market stall
// (tiny terracotta awning). Scale/rotation/offset vary per seed for asymmetry.
//
// PERFORMANCE: drawn ONCE into a single Graphics at setCityState time; never
// touched per frame. A hard cap bounds the total prop count.

import { Graphics } from "pixi.js";
import { cartToIso } from "./iso";
import { buildingFootprintTiles } from "./navWalkable";
import { DERIVED } from "./palette";
import { rngFromCoords, type Rng } from "./rng";
import type { TerrainExtent } from "./terrain";

const MAX_PROPS = 1500;

// Max props per Graphics chunk. Pixi v8 marks Graphics with ≥400 vertices as
// non-batchable (each shape primitive → a separate GL draw call). With ~4
// vertices per shape and ~7 shapes per prop, 80 props ≈ 2240 vertices → still
// large but the batcher merges these smaller chunks far more efficiently than
// one monolithic 1500-prop object.
const CHUNK_PROPS = 80;

// Per-kind base probability on an empty tile (before the cap). Tuned sparse so
// the ground stays mostly open with occasional clusters.
const P_OLIVE = 0.14;
const P_ROCK = 0.08;
const P_STALL = 0.02;

/** Build the set of occupied tile keys ("tx,ty") from building coords. */
export function occupiedTiles(coords: { x: number; y: number }[]): Set<string> {
  const set = new Set<string>();
  const footprints = buildingFootprintTiles(coords);
  for (const key of footprints) {
    // Unpack tileKey → (tx, ty) for the 4-neighbourhood expansion.
    const tx = (key >>> 16) - 0x8000;
    const ty = (key & 0xFFFF) - 0x8000;
    // Claim the tile and its 4-neighborhood so props don't crowd footprints.
    set.add(`${tx},${ty}`);
    set.add(`${tx + 1},${ty}`);
    set.add(`${tx - 1},${ty}`);
    set.add(`${tx},${ty + 1}`);
    set.add(`${tx},${ty - 1}`);
  }
  return set;
}

function drawOliveCluster(g: Graphics, cx: number, cy: number, rng: Rng): void {
  const blobs = rng.int(2, 4);
  for (let i = 0; i < blobs; i++) {
    const ox = rng.jitter(16);
    const oy = rng.jitter(8);
    const r = rng.range(3, 6.5);
    // Shadow under the bush.
    g.ellipse(cx + ox, cy + oy + r * 0.5, r * 0.9, r * 0.4).fill({
      color: DERIVED.oliveDark,
      alpha: 0.28,
    });
    // Body + a lit cap for a touch of volume.
    g.ellipse(cx + ox, cy + oy, r, r * 0.8).fill({
      color: rng.bool(0.5) ? DERIVED.olive : DERIVED.oliveDark,
      alpha: 0.95,
    });
    g.ellipse(cx + ox - r * 0.2, cy + oy - r * 0.25, r * 0.45, r * 0.35).fill({
      color: DERIVED.oliveLight,
      alpha: 0.7,
    });
  }
}

function drawRocks(g: Graphics, cx: number, cy: number, rng: Rng): void {
  const rocks = rng.int(1, 3);
  for (let i = 0; i < rocks; i++) {
    const ox = rng.jitter(18);
    const oy = rng.jitter(9);
    const s = rng.range(2.5, 5.5);
    const rot = rng.range(0, Math.PI);
    // An irregular 5-gon stone, rotated by seed for asymmetry.
    const pts: number[] = [];
    const n = 5;
    for (let k = 0; k < n; k++) {
      const a = rot + (k / n) * Math.PI * 2;
      const rr = s * (0.7 + (k % 2) * 0.4);
      pts.push(cx + ox + Math.cos(a) * rr, cy + oy + Math.sin(a) * rr * 0.55);
    }
    g.ellipse(cx + ox, cy + oy + s * 0.4, s, s * 0.4).fill({
      color: DERIVED.rockDark,
      alpha: 0.25,
    });
    g.poly(pts).fill({ color: rng.bool(0.5) ? DERIVED.rock : DERIVED.rockDark });
    // Lit facet.
    g.poly([
      cx + ox,
      cy + oy - s * 0.5,
      cx + ox + s * 0.5,
      cy + oy,
      cx + ox,
      cy + oy,
    ]).fill({ color: DERIVED.rockLight, alpha: 0.55 });
  }
}

function drawStall(g: Graphics, cx: number, cy: number, rng: Rng): void {
  // Tiny courtyard pad + a market awning on two posts.
  const w = rng.range(14, 20);
  const d = w * 0.5;
  g.poly([cx, cy - d, cx + w, cy, cx, cy + d, cx - w, cy]).fill({
    color: DERIVED.courtyard,
    alpha: 0.8,
  });
  // Posts.
  const ph = rng.range(8, 12);
  g.rect(cx - w * 0.5, cy - ph, 1.6, ph).fill({ color: DERIVED.pole });
  g.rect(cx + w * 0.5 - 1.6, cy - ph, 1.6, ph).fill({ color: DERIVED.pole });
  // Striped awning (two bands) — flips orientation by seed.
  const flip = rng.bool();
  const aw = w * 0.7;
  g.poly([
    cx - aw,
    cy - ph,
    cx + aw,
    cy - ph,
    cx + aw - 3,
    cy - ph - 5,
    cx - aw - 3,
    cy - ph - 5,
  ]).fill({ color: flip ? DERIVED.awning : DERIVED.awningDark, alpha: 0.95 });
  g.poly([
    cx - aw - 3,
    cy - ph - 5,
    cx + aw - 3,
    cy - ph - 5,
    cx + aw - 6,
    cy - ph - 9,
    cx - aw - 6,
    cy - ph - 9,
  ]).fill({ color: flip ? DERIVED.awningDark : DERIVED.awning, alpha: 0.9 });
}

/**
 * Draw decorative props on empty tiles within `ext`. Returns a FLAT array of
 * Graphics (caller owns destruction) plus the placed count.
 *
 * PERFORMANCE FIX (T6a): chunk props into ≤CHUNK_PROPS per Graphics so each
 * stays under Pixi v8's batchability threshold → the batcher merges them into
 * O(10) draw calls instead of one monolithic non-batchable object.
 */
export function drawProps(
  ext: TerrainExtent,
  occupied: Set<string>,
): { graphics: Graphics[]; propCount: number } {
  const chunks: Graphics[] = [];
  let g = new Graphics();
  let chunkCount = 0;
  let placed = 0;

  for (let ty = ext.minY; ty <= ext.maxY && placed < MAX_PROPS; ty++) {
    for (let tx = ext.minX; tx <= ext.maxX && placed < MAX_PROPS; tx++) {
      if (occupied.has(`${tx},${ty}`)) continue;
      const rng = rngFromCoords(tx, ty);
      const roll = rng.float();
      const c = cartToIso(tx, ty);
      // Small per-tile center jitter so clusters don't snap to a perfect grid.
      const cx = c.x + rng.jitter(20);
      const cy = c.y + rng.jitter(10);

      // Rotate to a fresh chunk when the current one is full.
      if (chunkCount >= CHUNK_PROPS) {
        chunks.push(g);
        g = new Graphics();
        chunkCount = 0;
      }

      if (roll < P_STALL) {
        drawStall(g, cx, cy, rng);
        placed++;
        chunkCount++;
      } else if (roll < P_STALL + P_ROCK) {
        drawRocks(g, cx, cy, rng);
        placed++;
        chunkCount++;
      } else if (roll < P_STALL + P_ROCK + P_OLIVE) {
        drawOliveCluster(g, cx, cy, rng);
        placed++;
        chunkCount++;
      }
    }
  }

  // Flush the last partial chunk.
  if (chunkCount > 0) chunks.push(g);

  return { graphics: chunks, propCount: placed };
}
