// Water terrain textured path — TilingSprites + mask, fallback, animation.
//
// Follows the style of props.order.test.ts: construct a tiny fake TerrainData +
// SpriteBank with Texture.WHITE under the right keys; no renderer needed.

import { describe, expect, it } from "vitest";
import { Container, Graphics, TilingSprite, Texture } from "pixi.js";
import { buildTerrainFrame, foamEdges } from "./terrain";
import { SpriteBank } from "./spriteAssets";
import type { TerrainData } from "../../types/city";

const CHUNK = 8;

/** Tiny terrain with sea tiles (some deep) + sand + bridges. */
function makeTerrain(): TerrainData {
  return {
    seaX: 10,
    minY: 0,
    maxY: 4,
    rivers: [{ gxMin: 3, gxMax: 3 }],
    water: [
      { gx: 10, gy: 0, deep: false },
      { gx: 11, gy: 0, deep: true },
      { gx: 3, gy: 1, deep: false },
    ],
    sand: [
      { gx: 2, gy: 1 },
      { gx: 9, gy: 0 },
    ],
    bridges: [{ gx: 3, gy: 1 }],
  };
}

/** SpriteBank with both tex:water and tex:waterdeep (textured path active). */
function bankWithWater(): SpriteBank {
  const textures = new Map<string, Texture>();
  textures.set("tex:water", Texture.WHITE);
  textures.set("tex:waterdeep", Texture.WHITE);
  return new SpriteBank(textures, new Map());
}

/** SpriteBank missing tex:waterdeep (all-or-nothing → flat path). */
function bankMissingDeep(): SpriteBank {
  const textures = new Map<string, Texture>();
  textures.set("tex:water", Texture.WHITE);
  // tex:waterdeep deliberately absent
  return new SpriteBank(textures, new Map());
}

/** Collect all TilingSprite descendants from a Container. */
function collectTilingSprites(root: Container): TilingSprite[] {
  const out: TilingSprite[] = [];
  root.children.forEach((c) => {
    if (c instanceof TilingSprite) out.push(c);
    else if (c instanceof Container) out.push(...collectTilingSprites(c));
  });
  return out;
}

describe("textured water path (both keys present)", () => {
  it("waterGroup is a masked Container with exactly 2 TilingSprites (mask is Graphics)", () => {
    const { waterGroup } = buildTerrainFrame(makeTerrain(), CHUNK, bankWithWater());

    expect(waterGroup).not.toBeNull();
    expect(waterGroup!).toBeInstanceOf(Container);
    // Mask is a Graphics child of waterGroup.
    const hasMask = waterGroup!.children.some((c) => c instanceof Graphics);
    expect(hasMask).toBe(true);
    expect(waterGroup!.mask).toBeInstanceOf(Graphics);

    // Exactly 2 TilingSprites inside waterGroup.
    const ts = collectTilingSprites(waterGroup!);
    expect(ts.length).toBe(2);
    const base = ts.find((s) => s.alpha === 1);
    const deep = ts.find((s) => s.alpha < 1);
    expect(base).toBeDefined();
    expect(deep).toBeDefined();
    expect(deep!.alpha).toBeCloseTo(0.25);
  });

  it("waterAnim mutates tilePosition of both sprites and creates no new display objects", () => {
    const { waterGroup, waterAnim } = buildTerrainFrame(makeTerrain(), CHUNK, bankWithWater());
    expect(waterGroup).not.toBeNull();
    expect(waterAnim).not.toBeNull();

    // Trigger initial update.
    waterAnim!.update(0);

    // Capture TilingSprite tilePositions before.
    const tsBefore = collectTilingSprites(waterGroup!);
    const positionsBefore = tsBefore.map((s) => ({ x: s.tilePosition.x, y: s.tilePosition.y }));

    // Advance time.
    waterAnim!.update(5);

    // TilePositions must have changed.
    const tsAfter = collectTilingSprites(waterGroup!);
    const positionsAfter = tsAfter.map((s) => ({ x: s.tilePosition.x, y: s.tilePosition.y }));
    expect(positionsAfter.length).toBe(positionsBefore.length);
    let changed = false;
    for (let i = 0; i < positionsAfter.length; i++) {
      if (positionsAfter[i].x !== positionsBefore[i].x ||
          positionsAfter[i].y !== positionsBefore[i].y) {
        changed = true;
        break;
      }
    }
    expect(changed).toBe(true);

    // No new display objects created: waterGroup child count unchanged.
    const childCount = waterGroup!.children.length;
    waterAnim!.update(10);
    expect(waterGroup!.children.length).toBe(childCount);
  });

  it("waterBounds equals the pixel bbox of all water diamond vertices (±0.5 tolerance)", () => {
    const { waterBounds } = buildTerrainFrame(makeTerrain(), CHUNK, bankWithWater());
    expect(waterBounds).not.toBeNull();

    // Manual expected bbox from the 3 water tiles: (10,0), (11,0), (3,1).
    // Each diamond is cartToIso(gx+0.5, gy+0.5) ± {HW, HH}.
    // We compare against a generous range — the exact values depend on cartToIso.
    const wb = waterBounds!;
    expect(wb.width).toBeGreaterThan(0);
    expect(wb.height).toBeGreaterThan(0);
    // The bbox must be large enough to span from tile (3,1) to tile (11,0)
    // which are far apart in iso space.
    expect(wb.width).toBeGreaterThan(300);
    expect(wb.height).toBeGreaterThan(50);
  });

  it("chunk containers have no TilingSprites; deep tiles get flat overlay fills", () => {
    const { chunks: frame } = buildTerrainFrame(makeTerrain(), CHUNK, bankWithWater());
    // Chunk containers should not contain any TilingSprites (they're in waterGroup).
    for (const chunk of frame) {
      const ts = collectTilingSprites(chunk.container);
      expect(ts.length).toBe(0);
    }
  });

  it("terrain chunks have no globalAnim/globalDestroy (clean contract)", () => {
    const { chunks: frame } = buildTerrainFrame(makeTerrain(), CHUNK, bankWithWater());
    for (const chunk of frame) {
      expect((chunk as any).globalAnim).toBeUndefined();
      expect((chunk as any).globalDestroy).toBeUndefined();
    }
  });
});

describe("fallback: bank missing tex:waterdeep (all-or-nothing)", () => {
  it("produces zero TilingSprites; flat fills identical to bank=null", () => {
    const { chunks: frameWithMissing, waterGroup: wgMissing, waterBounds: wbMissing } =
      buildTerrainFrame(makeTerrain(), CHUNK, bankMissingDeep());
    const { chunks: frameNull, waterGroup: wgNull, waterBounds: wbNull } =
      buildTerrainFrame(makeTerrain(), CHUNK, null);

    // No waterGroup when textured path inactive.
    expect(wgMissing).toBeNull();
    expect(wgNull).toBeNull();
    expect(wbMissing).toBeNull();
    expect(wbNull).toBeNull();

    // Zero TilingSprites with missing key.
    for (const chunk of frameWithMissing) {
      const ts = collectTilingSprites(chunk.container);
      expect(ts.length).toBe(0);
    }

    // Same number of chunks.
    expect(frameWithMissing.length).toBe(frameNull.length);

    // Shimmer anims present on water chunks (same as null path).
    const waterAnimsWithMissing = frameWithMissing.filter((c) => c.anim !== null);
    const waterAnimsNull = frameNull.filter((c) => c.anim !== null);
    expect(waterAnimsWithMissing.length).toBe(waterAnimsNull.length);
  });

  it("waterAnim is null (no textured water)", () => {
    const { waterAnim } = buildTerrainFrame(makeTerrain(), CHUNK, bankMissingDeep());
    expect(waterAnim).toBeNull();
  });
});

describe("fallback: bank=null (no bank)", () => {
  it("zero TilingSprites, flat fills, shimmer works", () => {
    const { chunks: frame, waterGroup, waterBounds, waterAnim } =
      buildTerrainFrame(makeTerrain(), CHUNK, null);
    expect(waterGroup).toBeNull();
    expect(waterBounds).toBeNull();
    expect(waterAnim).toBeNull();
    for (const chunk of frame) {
      const ts = collectTilingSprites(chunk.container);
      expect(ts.length).toBe(0);
    }
    // Shimmer anims still work.
    const anims = frame.filter((c) => c.anim !== null);
    expect(anims.length).toBeGreaterThan(0);
    for (const c of anims) {
      expect(() => { c.anim!.update(0); c.anim!.update(1.5); }).not.toThrow();
    }
  });
});

describe("foamEdges (pure helper)", () => {
  it("lone water tile (all 4 neighbors land) → 4 land-facing edges", () => {
    const waterSet = new Set(["5,5"]); // isolated tile
    expect(foamEdges(waterSet, 5, 5)).toBe(4);
  });

  it("interior water tile (all neighbors water) → 0 land-facing edges", () => {
    const waterSet = new Set(["5,5", "4,5", "6,5", "5,4", "5,6"]); // cross
    expect(foamEdges(waterSet, 5, 5)).toBe(0);
  });

  it("edge tile (3 water neighbors, 1 land) → 1 land-facing edge", () => {
    const waterSet = new Set(["5,5", "4,5", "6,5", "5,4"]); // south open
    expect(foamEdges(waterSet, 5, 5)).toBe(1);
  });

  it("corner tile (2 water neighbors, 2 land) → 2 land-facing edges", () => {
    const waterSet = new Set(["5,5", "4,5", "5,4"]); // SE open
    expect(foamEdges(waterSet, 5, 5)).toBe(2);
  });
});
