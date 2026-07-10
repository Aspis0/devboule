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

import { Graphics, Sprite, type Container } from "pixi.js";
import { cartToIso } from "./iso";
import { buildingFootprintTiles } from "./navWalkable";
import { DERIVED } from "./palette";
import { rngFromCoords, hashCoords, type Rng } from "./rng";
import type { SpriteBank } from "./spriteAssets";
import type { TerrainExtent } from "./terrain";

const MAX_PROPS = 2800;
// Raised cap for forest patches: forests add concentrated tree density that
// can push total props beyond the base cap. The cap is raised proportionally
// when forest patches exist (see planForestPatches).
const FOREST_EXTRA_PROPS_PER_PATCH = 120;

// Interleaved lattice passes (see drawProps): spreads a cap hit uniformly.
const SCAN_PHASES = 16;

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

// Forest patch constants.
const FOREST_PATCH_COUNT_MIN = 3;
const FOREST_PATCH_COUNT_MAX = 5;
const FOREST_RADIUS_MIN = 3;
const FOREST_RADIUS_MAX = 6;
const FOREST_LATTICE_STEP = 18; // spacing between candidate patch centres

// A4 — real tree sprites (UH maples/tupelos). Trees are TALL (≈2 tiles
// up-screen) and props live BELOW the buildings layer, so a tree drawn too
// close to a building would be wrongly covered by it. Trees therefore only
// spawn with a clear Chebyshev ring around them (countryside); inside the
// city the short procedural olive clusters keep working.
const TREE_CLEARANCE = 2;
// Among clearance-eligible olive rolls: chance the tile gets a sprite tree.
const P_TREE_GIVEN_OLIVE = 0.85;
// In a forest patch: boosted olive → tree probability (near-certain).
const P_TREE_GIVEN_OLIVE_FOREST = 0.98;
// Of the sprite trees: chance of the tall dark cypress (tupelo) variant.
const P_CYPRESS = 0.3;
// In a forest: boosted cypress proportion (more dark foliage variety).
const P_CYPRESS_FOREST = 0.45;

export interface ForestPatch {
  /** Centre tile of the patch. */
  cx: number;
  cy: number;
  /** Radius in tiles (Chebyshev). */
  radius: number;
}

/** True when every tile within `ring` Chebyshev distance is unoccupied. */
function hasClearance(
  occupied: Set<string>,
  tx: number,
  ty: number,
  ring: number,
): boolean {
  for (let dy = -ring; dy <= ring; dy++) {
    for (let dx = -ring; dx <= ring; dx++) {
      if (occupied.has(`${tx + dx},${ty + dy}`)) return false;
    }
  }
  return true;
}

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
 * Plan 3-5 forest patches on the countryside. Pure function of extent + blockers.
 * Deterministic: hash-based seeds from the lattice scan, no Math.random.
 *
 * Returns the list of patches and a raised MAX_PROPS that accommodates the
 * concentrated tree density without truncating the global prop cap.
 */
export function planForestPatches(
  ext: TerrainExtent,
  occupied: Set<string>,
): { patches: ForestPatch[]; cap: number } {
  // Candidate lattice: step 18 tiles gives ~4–6 candidates per map side.
  const cols = Math.max(1, Math.ceil((ext.maxX - ext.minX + 1) / FOREST_LATTICE_STEP));
  const rows = Math.max(1, Math.ceil((ext.maxY - ext.minY + 1) / FOREST_LATTICE_STEP));
  const n = cols * rows;

  const candidates: ForestPatch[] = [];
  for (let i = 0; i < n; i++) {
    const gx = ext.minX + (i % cols) * FOREST_LATTICE_STEP + FOREST_LATTICE_STEP / 2;
    const gy = ext.minY + Math.floor(i / cols) * FOREST_LATTICE_STEP + FOREST_LATTICE_STEP / 2;
    // Deterministic radius from hash.
    const h = hashCoords(gx, gy);
    const radius = FOREST_RADIUS_MIN + (h % (FOREST_RADIUS_MAX - FOREST_RADIUS_MIN + 1));
    // Check that the centre is not inside an occupied tile.
    if (occupied.has(`${Math.round(gx)},${Math.round(gy)}`)) continue;
    candidates.push({ cx: Math.round(gx), cy: Math.round(gy), radius });
  }

  // Pick patches deterministically: hash each candidate, sort by hash, take first N.
  candidates.sort((a, b) => {
    const ha = hashCoords(a.cx, a.cy);
    const hb = hashCoords(b.cx, b.cy);
    return ha - hb;
  });
  const count = Math.min(candidates.length, FOREST_PATCH_COUNT_MIN + ((hashCoords(ext.minX, ext.minY) % (FOREST_PATCH_COUNT_MAX - FOREST_PATCH_COUNT_MIN + 1))));
  const patches = candidates.slice(0, count);

  // Raised cap: each patch adds concentrated tree density.
  const cap = MAX_PROPS + patches.length * FOREST_EXTRA_PROPS_PER_PATCH;
  return { patches, cap };
}

/** True when (tx, ty) is inside any forest patch (Chebyshev distance). */
function inForestPatch(patches: ForestPatch[], tx: number, ty: number): boolean {
  for (const p of patches) {
    const dx = Math.abs(tx - p.cx);
    const dy = Math.abs(ty - p.cy);
    if (dx <= p.radius && dy <= p.radius) return true;
  }
  return false;
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
  bank?: SpriteBank | null,
  // Tiles that block TALL sprites' clearance ring (buildings only — field
  // parcels are flat, so trees standing over them are z-safe and read as
  // hedgerows). Defaults to `occupied` when the caller doesn't split them.
  tallBlockers: Set<string> = occupied,
  // Optional pre-planned forest patches. When provided and sprites are enabled,
  // tree density is boosted inside patches. Passed from the renderer.
  forestPatches?: ForestPatch[] | null,
  // Optional cap override: when provided (e.g. from planForestPatches), used
  // instead of the default base cap. This is the single source of truth for
  // the prop count limit when forest patches are active.
  capOverride?: number,
): { graphics: (Graphics | Container)[]; propCount: number } {
  const chunks: (Graphics | Container)[] = [];
  const cap = capOverride ?? MAX_PROPS;
  // MAX-RECALL fix — tree Sprites are collected SEPARATELY and appended after
  // every Graphics chunk: interleaving them (all on the prop-0 atlas page)
  // between singles-textured Graphics broke the batch per tree — one draw
  // call each. As one contiguous block the batcher merges them; being flat
  // decoration below every tree, the ground chunks may safely all paint
  // first. Within the block, painter's order = ascending y (a south tree
  // must cover a north one).
  const treeSprites: Sprite[] = [];
  let g = new Graphics();
  // Draw ops in the CURRENT Graphics chunk only — sprite trees bypass `g`
  // (they're standalone children), so counting them here would rotate
  // under-filled (or empty) Graphics chunks: wasted draw calls.
  let chunkCount = 0;
  let placed = 0;

  // Interleaved passes over the tile lattice: a MAX_PROPS cap hit thins
  // density uniformly across the whole map instead of stripping the south
  // (row-major left everything below the first ~1500 candidates bare).
  const cols = ext.maxX - ext.minX + 1;
  const rows = ext.maxY - ext.minY + 1;
  const n = cols * rows;
  for (let phase = 0; phase < SCAN_PHASES && placed < cap; phase++) {
    for (let i = phase; i < n && placed < cap; i += SCAN_PHASES) {
      const tx = ext.minX + (i % cols);
      const ty = ext.minY + Math.floor(i / cols);
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
        // Real tree sprite when the bank has one AND the tile sits in open
        // countryside (see TREE_CLEARANCE); otherwise the classic olive blobs.
        // MAX-RECALL fix — the tree/cypress rolls are drawn UNCONDITIONALLY
        // so the per-tile rng stream is identical with and without a bank:
        // the procedural fallback cluster must not change shape based on
        // whether the sprite load happened to win its 3s race.
        const treeRoll = rng.float();
        const cypressRoll = rng.float();
        const inForest = forestPatches != null && forestPatches.length > 0
          ? inForestPatch(forestPatches, tx, ty)
          : false;
        // Forest patches boost tree probability and cypress ratio — more trees,
        // more dark foliage variety inside the patch radius.
        const treeProb = inForest ? P_TREE_GIVEN_OLIVE_FOREST : P_TREE_GIVEN_OLIVE;
        const cypressProb = inForest ? P_CYPRESS_FOREST : P_CYPRESS;
        let treeKey: string | null = null;
        if (
          bank &&
          treeRoll < treeProb &&
          hasClearance(tallBlockers, tx, ty, TREE_CLEARANCE)
        ) {
          const family = cypressRoll < cypressProb ? "prop:cypress" : "prop:tree";
          treeKey = bank.pickVariant(family, `${tx},${ty}`);
        }
        const texture = treeKey ? bank!.get(treeKey) : null;
        if (treeKey && texture) {
          const sprite = new Sprite(texture);
          const [ax, ay] = bank!.anchor(treeKey);
          sprite.anchor.set(ax, ay);
          // Base at the tile-center ground point (same spot the olive
          // cluster uses); slight seeded scale variety keeps rows organic.
          sprite.position.set(cx, cy + 6);
          const s = rng.range(0.85, 1.1);
          sprite.scale.set(s, s);
          treeSprites.push(sprite); // NOT in `g` — doesn't advance chunkCount
        } else {
          drawOliveCluster(g, cx, cy, rng);
          chunkCount++;
        }
        placed++;
      }
    }
  }

  // Flush the last partial chunk, then the tree block (see treeSprites note).
  if (chunkCount > 0) chunks.push(g);
  treeSprites.sort((a, b) => a.position.y - b.position.y);
  for (const s of treeSprites) chunks.push(s);

  return { graphics: chunks, propCount: placed };
}
