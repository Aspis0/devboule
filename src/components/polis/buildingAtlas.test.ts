// BuildingTextureAtlas — lazy per-variant texture cache contract.
//
// Proves the atlas's pure logic without a GPU: a fake TextureSource returns a stub
// Texture and counts generateTexture calls; a fake `build` closure returns trivial
// real PIXI display objects so the atlas can measure bounds + destroy them.
//
//   - KEY: two buildings with the SAME (purpose, level, salt) share ONE texture;
//     a different level / purpose / salt yields a DIFFERENT texture.
//   - SALT: same fileId → same salt; different fileIds spread across N variants;
//     profile clamp (lean 2 / minimal 1); landmark presence scale pure function.
//   - LAZY: nothing is generated until the first `get`; `has` is false beforehand.
//   - SHARED: a cache HIT does NOT call the `build` closure or generateTexture again.
//   - DESTROY: `destroy()` releases every cached texture and empties the cache.
//   - SOURCE DISPOSAL: on a MISS the atlas destroys the source body/shadow it was
//     handed (so the heavy static Graphics is never retained).

import { describe, it, expect, vi } from "vitest";

const { Container, Graphics, Texture } = await import("pixi.js");
const {
  BuildingTextureAtlas,
  variantKey,
  atlasResolution,
  ATLAS_RESOLUTION_CAP,
  baseVariantCount,
  variantCountFor,
  buildingSalt,
  landmarkPresenceScale,
} = await import("./buildingAtlas");
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
  build: () => {
    body: InstanceType<typeof Container>;
    shadow: InstanceType<typeof Graphics>;
    foot: [number, number];
  };
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
    return { body, shadow, foot: [2, 2] as [number, number] };
  });
  return { build: fn, count: () => fn.mock.calls.length, bodies, shadows };
}

describe("variantKey", () => {
  it("is purpose:level:sN and distinguishes purpose, level, and salt", () => {
    expect(variantKey("house", 2)).toBe("house:2:s0");
    expect(variantKey("house", 2, 3)).toBe("house:2:s3");
    expect(variantKey("house", 2, 0)).not.toBe(variantKey("house", 2, 1));
    expect(variantKey("house", 2)).not.toBe(variantKey("house", 3));
    expect(variantKey("house", 2)).not.toBe(variantKey("temple", 2));
  });
});

describe("baseVariantCount / variantCountFor — purpose class + profile clamp", () => {
  it("assigns N by purpose class", () => {
    expect(baseVariantCount("house")).toBe(4);
    expect(baseVariantCount("workshop")).toBe(4);
    expect(baseVariantCount("warehouse")).toBe(4);
    expect(baseVariantCount("unknown")).toBe(4);
    expect(baseVariantCount("market")).toBe(2);
    expect(baseVariantCount("library")).toBe(2);
    expect(baseVariantCount("baths")).toBe(2);
    expect(baseVariantCount("theater")).toBe(2);
    expect(baseVariantCount("townhall")).toBe(2);
    expect(baseVariantCount("temple")).toBe(1);
    expect(baseVariantCount("lighthouse")).toBe(1);
    expect(baseVariantCount("fortress")).toBe(1);
    expect(baseVariantCount("harbor")).toBe(1);
    expect(baseVariantCount("conduit")).toBe(1);
    expect(baseVariantCount("tower")).toBe(1);
    // fallback purpose slug → high-frequency default
    expect(baseVariantCount("some-new-purpose")).toBe(4);
  });

  it("clamps N by profile saltMax (rich 4 / lean 2 / minimal 1)", () => {
    expect(variantCountFor("house", 4)).toBe(4);
    expect(variantCountFor("house", 2)).toBe(2); // lean
    expect(variantCountFor("house", 1)).toBe(1); // minimal
    expect(variantCountFor("market", 4)).toBe(2); // base already 2
    expect(variantCountFor("market", 2)).toBe(2);
    expect(variantCountFor("market", 1)).toBe(1);
    expect(variantCountFor("temple", 4)).toBe(1); // landmark stays 1
    expect(variantCountFor("temple", 1)).toBe(1);
  });
});

describe("buildingSalt — determinism + distribution", () => {
  it("same fileId → same salt; is stable across calls", () => {
    const a = buildingSalt("src/foo.ts", "house", 4);
    const b = buildingSalt("src/foo.ts", "house", 4);
    expect(a).toBe(b);
    expect(a).toBeGreaterThanOrEqual(0);
    expect(a).toBeLessThan(4);
  });

  it("different fileIds spread across variants for high-frequency purposes", () => {
    const salts = new Set<number>();
    for (let i = 0; i < 64; i++) {
      salts.add(buildingSalt(`file-${i}.ts`, "house", 4));
    }
    // With a decent hash, 64 samples of N=4 should hit every bucket.
    expect(salts.size).toBe(4);
    for (const s of salts) {
      expect(s).toBeGreaterThanOrEqual(0);
      expect(s).toBeLessThan(4);
    }
  });

  it("distribution over N is reasonable for a sample of 200 ids", () => {
    const counts = [0, 0, 0, 0];
    const N = 4;
    const SAMPLE = 200;
    for (let i = 0; i < SAMPLE; i++) {
      counts[buildingSalt(`path/to/file_${i}.ts`, "house", N)]++;
    }
    // No bucket should be empty or monopolise > half the sample.
    for (const c of counts) {
      expect(c).toBeGreaterThan(0);
      expect(c).toBeLessThan(SAMPLE * 0.5);
    }
  });

  it("landmarks always salt 0; lean/minimal clamp range", () => {
    expect(buildingSalt("any-id", "temple", 4)).toBe(0);
    expect(buildingSalt("any-id", "lighthouse", 4)).toBe(0);
    // minimal: N=1 → always 0 even for house
    expect(buildingSalt("a", "house", 1)).toBe(0);
    expect(buildingSalt("b", "house", 1)).toBe(0);
    // lean: N=2 → salt in {0,1}
    for (let i = 0; i < 32; i++) {
      const s = buildingSalt(`lean-${i}`, "house", 2);
      expect(s === 0 || s === 1).toBe(true);
    }
  });
});

describe("landmarkPresenceScale", () => {
  it("scales civic/rare purposes modestly and leaves ordinary purposes at 1", () => {
    expect(landmarkPresenceScale("temple")).toBeGreaterThanOrEqual(1.06);
    expect(landmarkPresenceScale("temple")).toBeLessThanOrEqual(1.1);
    expect(landmarkPresenceScale("theater")).toBeGreaterThanOrEqual(1.06);
    expect(landmarkPresenceScale("library")).toBeGreaterThanOrEqual(1.06);
    expect(landmarkPresenceScale("baths")).toBeGreaterThanOrEqual(1.06);
    expect(landmarkPresenceScale("lighthouse")).toBeGreaterThanOrEqual(1.06);
    expect(landmarkPresenceScale("lighthouse")).toBeLessThanOrEqual(1.1);
    // ordinary / other
    expect(landmarkPresenceScale("house")).toBe(1);
    expect(landmarkPresenceScale("workshop")).toBe(1);
    expect(landmarkPresenceScale("fortress")).toBe(1);
    expect(landmarkPresenceScale("harbor")).toBe(1);
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
    expect(atlas.has("house", 2, 1)).toBe(false);
    expect(calls()).toBe(0);
  });

  it("KEY: same (purpose, level, salt) → ONE shared texture; build/generate once", () => {
    const atlas = new BuildingTextureAtlas(1);
    const { src, calls } = fakeRenderer();
    const { build, count } = fakeBuild();

    const a = atlas.get(src, "house", 2, build, 1);
    const b = atlas.get(src, "house", 2, build, 1);

    // Same shared texture object for both buildings of the variant.
    expect(b.texture).toBe(a.texture);
    expect(b.shadowTexture).toBe(a.shadowTexture);
    // Build closure ran ONCE; generateTexture ran twice for the FIRST build only
    // (body + shadow), not again on the hit.
    expect(count()).toBe(1);
    expect(calls()).toBe(2);
    expect(atlas.size).toBe(1);
    expect(atlas.has("house", 2, 1)).toBe(true);
    // Different salt is a MISS on has()
    expect(atlas.has("house", 2, 0)).toBe(false);
  });

  it("KEY: a different level, purpose, OR salt → a DIFFERENT texture", () => {
    const atlas = new BuildingTextureAtlas(1);
    const { src } = fakeRenderer();
    const { build } = fakeBuild();

    const h2s0 = atlas.get(src, "house", 2, build, 0);
    const h2s1 = atlas.get(src, "house", 2, build, 1);
    const h3 = atlas.get(src, "house", 3, build, 0);
    const t2 = atlas.get(src, "temple", 2, build, 0);

    expect(h2s1.texture).not.toBe(h2s0.texture);
    expect(h3.texture).not.toBe(h2s0.texture);
    expect(t2.texture).not.toBe(h2s0.texture);
    expect(atlas.size).toBe(4);
  });

  it("SOURCE DISPOSAL: the source body + shadow are destroyed on a MISS (no retention)", () => {
    const atlas = new BuildingTextureAtlas(1);
    const { src } = fakeRenderer();
    const { build, bodies, shadows } = fakeBuild();

    atlas.get(src, "house", 2, build, 0);

    expect(bodies[0].destroyed).toBe(true);
    expect(shadows[0].destroyed).toBe(true);
  });

  it("DESTROY: releases every cached texture and empties the cache", () => {
    const atlas = new BuildingTextureAtlas(1);
    const { src } = fakeRenderer();
    const { build } = fakeBuild();

    const v = atlas.get(src, "house", 2, build, 0);
    atlas.get(src, "temple", 4, build, 0);
    expect(atlas.size).toBe(2);

    atlas.destroy();

    expect(atlas.size).toBe(0);
    expect(v.texture.destroyed).toBe(true);
    expect(v.shadowTexture.destroyed).toBe(true);
    // After destroy the atlas rebuilds lazily (cache empty again).
    expect(atlas.has("house", 2, 0)).toBe(false);
  });
});
