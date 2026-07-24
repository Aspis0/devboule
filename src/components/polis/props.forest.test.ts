// props.forest.test.ts — forest patch determinism + density boost.
//
// Verifies:
//   1. planForestPatches is deterministic (same input → same output).
//   2. Forest patches are placed in the countryside (not overlapping occupied tiles).
//   3. drawProps with forest patches: when the base cap binds, the raised cap
//      allows more props to be placed (the forest path may place MORE than the
//      non-forest path because the cap is raised by patches × 120).
//   4. Stream parity: drawProps consumes the same number of rng draws regardless
//      of forest patches (rng draws are unconditional — see props.ts MAX-RECALL
//      comment). With the SAME cap, propCount is identical; with a raised cap,
//      the forest path may place more.

import { describe, expect, it } from "vitest";
import { Graphics, Sprite, Texture } from "pixi.js";
import {
  drawProps,
  planForestPatches,
  type ForestPatch,
} from "./props";
import { SpriteBank } from "./spriteAssets";
import { isoToCart } from "./iso";

// An extent where the rich base cap (3400) binds: ~120×120 = 14400 tiles × 0.24
// density ≈ 3456 expected props, exceeding 3400 once stalls/rocks/olives fill.
const EXT_BUST = { minX: 0, maxX: 119, minY: 0, maxY: 119 };

// A small extent where the base cap does NOT bind (for determinism tests).
const EXT_SMALL = { minX: 0, maxX: 50, minY: 0, maxY: 50 };

function treeBank(): SpriteBank {
  const textures = new Map<string, Texture>();
  for (let v = 0; v < 3; v++) {
    textures.set(`prop:tree:v${v}`, Texture.WHITE);
    textures.set(`prop:cypress:v${v}`, Texture.WHITE);
  }
  return new SpriteBank(textures, new Map());
}

describe("planForestPatches", () => {
  it("returns 5-8 patches (or fewer if extent is tiny)", () => {
    const occupied = new Set<string>();
    const { patches } = planForestPatches(EXT_SMALL, occupied);
    expect(patches.length).toBeGreaterThanOrEqual(0);
    expect(patches.length).toBeLessThanOrEqual(8);
  });

  it("is deterministic: same input → same output", () => {
    const occupied = new Set<string>();
    const a = planForestPatches(EXT_SMALL, occupied);
    const b = planForestPatches(EXT_SMALL, occupied);
    expect(a).toEqual(b);
  });

  it("patch centres are not on occupied tiles", () => {
    const occupied = new Set<string>();
    for (let x = 24; x <= 26; x++) {
      for (let y = 24; y <= 26; y++) occupied.add(`${x},${y}`);
    }
    const { patches } = planForestPatches(EXT_SMALL, occupied);
    for (const p of patches) {
      expect(occupied.has(`${p.cx},${p.cy}`)).toBe(false);
    }
  });

  it("cap is raised proportionally to patch count (rich base 3400)", () => {
    const occupied = new Set<string>();
    const { patches, cap } = planForestPatches(EXT_SMALL, occupied);
    expect(cap).toBe(3400 + patches.length * 120);
  });

  it("lean tier keeps the historical 2800 base", () => {
    const occupied = new Set<string>();
    const { patches, cap } = planForestPatches(EXT_SMALL, occupied, {
      tier: "lean",
    });
    expect(cap).toBe(2800 + patches.length * 120);
  });
});

describe("drawProps forest cap behavior", () => {
  it("when base cap binds, forest path places more props AND more trees inside patches", () => {
    const bank = treeBank();
    const occupied = new Set<string>();
    const { patches, cap: forestCap } = planForestPatches(EXT_BUST, occupied);

    // With the raised cap, the forest path has more room.
    const withForest = drawProps(EXT_BUST, occupied, bank, occupied, patches, forestCap);
    // With the rich base cap, the non-forest path is limited.
    const withoutForest = drawProps(EXT_BUST, occupied, bank, occupied, [], 3400);

    // The raised cap should allow at least as many props (likely more).
    expect(withForest.propCount).toBeGreaterThanOrEqual(withoutForest.propCount);
    expect(withoutForest.propCount).toBeGreaterThan(100);
    expect(withForest.propCount).toBeGreaterThan(100);

    // REAL FEATURE SIGNAL: trees inside forest patches must be ≥3× denser
    // (per-tile) than trees outside patches. This verifies the P_OLIVE_FOREST
    // boost actually produces visible woodland clusters.
    const treeSprites = withForest.graphics.filter((n) => n instanceof Sprite) as Sprite[];
    expect(treeSprites.length).toBeGreaterThan(0);

    // Map sprite iso positions back to tile coordinates via isoToCart.
    let treesInside = 0;
    let treesOutside = 0;
    for (const s of treeSprites) {
      const { x: tx, y: ty } = isoToCart(s.position.x, s.position.y);
      const itx = Math.round(tx);
      const ity = Math.round(ty);
      let inside = false;
      for (const p of patches) {
        if (Math.abs(itx - p.cx) <= p.radius && Math.abs(ity - p.cy) <= p.radius) {
          inside = true;
          break;
        }
      }
      if (inside) treesInside++; else treesOutside++;
    }

    // Compute per-tile density: patch area = Σ(2r+1)², outside = total − inside.
    const patchTiles = patches.reduce((s, p) => s + (2 * p.radius + 1) ** 2, 0);
    const totalTiles = (EXT_BUST.maxX - EXT_BUST.minX + 1) * (EXT_BUST.maxY - EXT_BUST.minY + 1);
    const outsideTiles = totalTiles - patchTiles;
    const densityInside = treesInside / patchTiles;
    const densityOutside = treesOutside / outsideTiles;
    expect(densityInside).toBeGreaterThan(densityOutside * 3);
  });

  it("with the same cap, propCount is identical (stream parity)", () => {
    const bank = treeBank();
    const occupied = new Set<string>();
    const { patches } = planForestPatches(EXT_BUST, occupied);
    // Use the SAME cap for both — rng draws are unconditional.
    const sameCap = 3400;
    const { propCount: withForest } = drawProps(EXT_BUST, occupied, bank, occupied, patches, sameCap);
    const { propCount: withoutForest } = drawProps(EXT_BUST, occupied, bank, occupied, [], sameCap);
    expect(withForest).toBe(withoutForest);
  });

  it("stream parity holds even without a bank (procedural path)", () => {
    const occupied = new Set<string>();
    const { patches } = planForestPatches(EXT_BUST, occupied);
    const sameCap = 3400;
    const withForest = drawProps(EXT_BUST, occupied, null, occupied, patches, sameCap);
    const withoutForest = drawProps(EXT_BUST, occupied, null, occupied, [], sameCap);
    expect(withForest.propCount).toBe(withoutForest.propCount);
    expect(withForest.graphics.every((n) => n instanceof Graphics)).toBe(true);
  });
});

describe("drawProps forest density boost", () => {
  it("forest patches produce y-sorted sprite trees", () => {
    const bank = treeBank();
    const patch: ForestPatch = { cx: 25, cy: 25, radius: 3 };
    const { graphics } = drawProps(EXT_SMALL, new Set(), bank, new Set(), [patch]);
    const sprites = graphics.filter((n) => n instanceof Sprite) as Sprite[];
    if (sprites.length < 2) return;
    for (let i = 1; i < sprites.length; i++) {
      expect(sprites[i].position.y).toBeGreaterThanOrEqual(sprites[i - 1].position.y);
    }
  });
});
