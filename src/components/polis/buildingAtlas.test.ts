// BuildingTextureAtlas — lazy per-variant texture cache contract.
//
// Proves the atlas's pure logic without a GPU: a fake TextureSource returns a stub
// Texture and counts generateTexture calls; a fake `build` closure returns trivial
// real PIXI display objects so the atlas can measure bounds + destroy them.
//
//   - KEY: two buildings with the SAME (purpose, level) share ONE texture object;
//     a different level (or purpose) yields a DIFFERENT texture (key includes every
//     static visual axis the kit varies on).
//   - LAZY: nothing is generated until the first `get`; `has` is false beforehand.
//   - SHARED: a cache HIT does NOT call the `build` closure or generateTexture again.
//   - DESTROY: `destroy()` releases every cached texture and empties the cache.
//   - SOURCE DISPOSAL: on a MISS the atlas destroys the source body/shadow it was
//     handed (so the heavy static Graphics is never retained).

import { describe, it, expect, vi } from "vitest";

const { Container, Graphics, Texture } = await import("pixi.js");
const { BuildingTextureAtlas, variantKey, atlasResolution, ATLAS_RESOLUTION_CAP } =
  await import("./buildingAtlas");
import type { TextureSource } from "./buildingAtlas";

/** A fake renderer: every generateTexture returns a FRESH stub Texture so identity
 *  comparisons are meaningful, and the call count is observable. */
function fakeRenderer(): { src: TextureSource; calls: () => number } {
  const gen = vi.fn(() => new Texture());
  return {
    src: { generateTexture: gen } as unknown as TextureSource,
    calls: () => gen.mock.calls.length,
  };
}

/** A fake `build` closure: returns trivial real PIXI objects (so getLocalBounds +
 *  destroy work) and counts how many times it ran. */
function fakeBuild(): {
  build: () => { body: InstanceType<typeof Container>; shadow: InstanceType<typeof Graphics> };
  count: () => number;
  bodies: InstanceType<typeof Container>[];
  shadows: InstanceType<typeof Graphics>[];
} {
  const bodies: InstanceType<typeof Container>[] = [];
  const shadows: InstanceType<typeof Graphics>[] = [];
  const fn = vi.fn(() => {
    const body = new Container();
    const g = new Graphics();
    g.rect(0, 0, 10, 10).fill(0xffffff); // give it real bounds
    body.addChild(g);
    const shadow = new Graphics();
    shadow.ellipse(0, 0, 8, 4).fill(0x000000);
    bodies.push(body);
    shadows.push(shadow);
    return { body, shadow };
  });
  return { build: fn, count: () => fn.mock.calls.length, bodies, shadows };
}

describe("variantKey", () => {
  it("is purpose:level and distinguishes both axes", () => {
    expect(variantKey("house", 2)).toBe("house:2");
    expect(variantKey("house", 2)).not.toBe(variantKey("house", 3));
    expect(variantKey("house", 2)).not.toBe(variantKey("temple", 2));
  });
});

describe("atlasResolution", () => {
  it("honours dpr but caps it and floors at 1", () => {
    expect(atlasResolution(1)).toBe(1);
    expect(atlasResolution(2)).toBe(ATLAS_RESOLUTION_CAP);
    expect(atlasResolution(4)).toBe(ATLAS_RESOLUTION_CAP); // capped
    expect(atlasResolution(0.5)).toBe(1); // floored
    expect(atlasResolution(0)).toBe(1); // invalid → 1
    expect(atlasResolution(NaN)).toBe(1);
  });
});

describe("BuildingTextureAtlas — lazy + keyed + shared", () => {
  it("is LAZY: nothing generated before the first request", () => {
    const atlas = new BuildingTextureAtlas(1);
    const { calls } = fakeRenderer();
    expect(atlas.size).toBe(0);
    expect(atlas.has("house", 2)).toBe(false);
    expect(calls()).toBe(0);
  });

  it("KEY: same (purpose, level) → ONE shared texture object; build/generate run ONCE", () => {
    const atlas = new BuildingTextureAtlas(1);
    const { src, calls } = fakeRenderer();
    const { build, count } = fakeBuild();

    const a = atlas.get(src, "house", 2, build);
    const b = atlas.get(src, "house", 2, build);

    // Same shared texture object for both buildings of the variant.
    expect(b.texture).toBe(a.texture);
    expect(b.shadowTexture).toBe(a.shadowTexture);
    // Build closure ran ONCE; generateTexture ran twice for the FIRST build only
    // (body + shadow), not again on the hit.
    expect(count()).toBe(1);
    expect(calls()).toBe(2);
    expect(atlas.size).toBe(1);
    expect(atlas.has("house", 2)).toBe(true);
  });

  it("KEY: a different level (or purpose) → a DIFFERENT texture", () => {
    const atlas = new BuildingTextureAtlas(1);
    const { src } = fakeRenderer();
    const { build } = fakeBuild();

    const h2 = atlas.get(src, "house", 2, build);
    const h3 = atlas.get(src, "house", 3, build);
    const t2 = atlas.get(src, "temple", 2, build);

    expect(h3.texture).not.toBe(h2.texture);
    expect(t2.texture).not.toBe(h2.texture);
    expect(atlas.size).toBe(3);
  });

  it("SOURCE DISPOSAL: the source body + shadow are destroyed on a MISS (no retention)", () => {
    const atlas = new BuildingTextureAtlas(1);
    const { src } = fakeRenderer();
    const { build, bodies, shadows } = fakeBuild();

    atlas.get(src, "house", 2, build);

    expect(bodies[0].destroyed).toBe(true);
    expect(shadows[0].destroyed).toBe(true);
  });

  it("DESTROY: releases every cached texture and empties the cache", () => {
    const atlas = new BuildingTextureAtlas(1);
    const { src } = fakeRenderer();
    const { build } = fakeBuild();

    const v = atlas.get(src, "house", 2, build);
    atlas.get(src, "temple", 4, build);
    expect(atlas.size).toBe(2);

    atlas.destroy();

    expect(atlas.size).toBe(0);
    expect(v.texture.destroyed).toBe(true);
    expect(v.shadowTexture.destroyed).toBe(true);
    // After destroy the atlas rebuilds lazily (cache empty again).
    expect(atlas.has("house", 2)).toBe(false);
  });
});
