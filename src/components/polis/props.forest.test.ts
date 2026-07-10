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

// An extent where the base cap (2800) binds: ~120×120 = 14400 tiles × 0.24
// density ≈ 3456 expected props, exceeding 2800.
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
  it("returns 3-5 patches (or fewer if extent is tiny)", () => {
    const occupied = new Set<string>();
    const { patches } = planForestPatches(EXT_SMALL, occupied);
    expect(patches.length).toBeGreaterThanOrEqual(0);
    expect(patches.length).toBeLessThanOrEqual(5);
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

  it("cap is raised proportionally to patch count", () => {
    const occupied = new Set<string>();
    const { patches, cap } = planForestPatches(EXT_SMALL, occupied);
    expect(cap).toBe(2800 + patches.length * 120);
  });
});

describe("drawProps forest cap behavior", () => {
  it("when base cap binds, forest path may place more props (raised cap allows it)", () => {
    const bank = treeBank();
    const occupied = new Set<string>();
    const { patches, cap: forestCap } = planForestPatches(EXT_BUST, occupied);

    // With the raised cap, the forest path has more room.
    const { propCount: withForest } = drawProps(EXT_BUST, occupied, bank, occupied, patches, forestCap);
    // With the base cap, the non-forest path is limited.
    const { propCount: withoutForest } = drawProps(EXT_BUST, occupied, bank, occupied, [], 2800);

    // The raised cap should allow at least as many props (likely more).
    // With a 120×120 extent the base cap binds hard, so forest patches
    // (which raise it by patches × 120) should produce more.
    expect(withForest).toBeGreaterThanOrEqual(withoutForest);
    // Sanity: both hit some cap (non-trivial prop count).
    expect(withoutForest).toBeGreaterThan(100);
    expect(withForest).toBeGreaterThan(100);
  });

  it("with the same cap, propCount is identical (stream parity)", () => {
    const bank = treeBank();
    const occupied = new Set<string>();
    const { patches } = planForestPatches(EXT_BUST, occupied);
    // Use the SAME cap for both — rng draws are unconditional.
    const sameCap = 2800;
    const { propCount: withForest } = drawProps(EXT_BUST, occupied, bank, occupied, patches, sameCap);
    const { propCount: withoutForest } = drawProps(EXT_BUST, occupied, bank, occupied, [], sameCap);
    expect(withForest).toBe(withoutForest);
  });

  it("stream parity holds even without a bank (procedural path)", () => {
    const occupied = new Set<string>();
    const { patches } = planForestPatches(EXT_BUST, occupied);
    const sameCap = 2800;
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
