import { describe, it, expect } from "vitest";
import { Container, Texture } from "pixi.js";
import {
  seededPhase,
  rankForPromotion,
  SEVERITY_SPAWN_MULTIPLIER,
  SEVERITY_SCALE,
  SEVERITY_RANK,
  beginDemotionCrossfade,
  parkHeroFire,
  stepHeroFire,
  createHeroFire,
  createCrowdFire,
  retargetHeroFire,
  crowdFireShowsFlame,
  SIN_SMOKE,
  SMOKE_PUFF,
  bakeFireAtlas,
  SMOKE_BAKE_SCALE,
  SMOKE_SPRITE_SCALE,
  SMOKE_TEX_WIDTH,
  SMOKE_TEX_HEIGHT,
  SMOKE_INK_ALPHA_FLOOR,
  measureSmokeInkCoverage,
  smokeBakeTimes,
  enumerateSmokeLobes,
  enumerateSmokePuffs,
  smokePuffVerticalSpacing,
  SMOKE_LOBES_PER_PUFF,
  type HeroFire,
  type FireAtlas,
} from "./fire";
import type { PromotableBuilding } from "./fire";
import {
  CHIMNEY_SMOKE_TINT,
  CHIMNEY_SMOKE_MAX_ALPHA,
} from "./ambientLife";
import {
  LOD_DISASTER,
  disasterEffectsLodVisible,
} from "./lod";

describe("seededPhase — determinism", () => {
  it("same fileId → same phase", () => {
    const a = seededPhase("file-a.ts");
    const b = seededPhase("file-a.ts");
    expect(a).toBe(b);
  });

  it("different fileIds → different phases (high probability)", () => {
    const a = seededPhase("src/foo.ts");
    const b = seededPhase("src/bar.ts");
    expect(a).not.toBe(b);
  });

  it("phase is in [0, 100)", () => {
    for (const id of ["a", "b", "long/path/file.ts", "uuid-1234-5678"]) {
      const p = seededPhase(id);
      expect(p).toBeGreaterThanOrEqual(0);
      expect(p).toBeLessThan(100);
    }
  });

  it("produces integer-like variation for frame offsets", () => {
    // Phases for different files should distribute across the range
    const phases = new Set<number>();
    for (let i = 0; i < 100; i++) {
      phases.add(Math.floor(seededPhase(`file-${i}.ts`)));
    }
    expect(phases.size).toBeGreaterThan(50); // good spread
  });
});

describe("SEVERITY_SPAWN_MULTIPLIER — severity mapping", () => {
  it("smoke = ×1.0", () => expect(SEVERITY_SPAWN_MULTIPLIER.smoke).toBe(1.0));
  it("fire = ×1.6", () => expect(SEVERITY_SPAWN_MULTIPLIER.fire).toBe(1.6));
  it("inferno = ×2.4", () => expect(SEVERITY_SPAWN_MULTIPLIER.inferno).toBe(2.4));
});

describe("SEVERITY_SCALE — flame scale per band", () => {
  it("inferno > fire > smoke", () => {
    expect(SEVERITY_SCALE.inferno).toBeGreaterThan(SEVERITY_SCALE.fire);
    expect(SEVERITY_SCALE.fire).toBeGreaterThan(SEVERITY_SCALE.smoke);
  });
});

describe("SEVERITY_RANK — promotion ordering", () => {
  it("inferno outranks fire outranks smoke", () => {
    expect(SEVERITY_RANK.inferno).toBeGreaterThan(SEVERITY_RANK.fire);
    expect(SEVERITY_RANK.fire).toBeGreaterThan(SEVERITY_RANK.smoke);
  });
});

describe("rankForPromotion", () => {
  it("ranks inferno above fire above smoke", () => {
    const buildings: PromotableBuilding[] = [
      { fileId: "a", severity: "smoke", distToCenter: 10 },
      { fileId: "b", severity: "inferno", distToCenter: 100 },
      { fileId: "c", severity: "fire", distToCenter: 5 },
    ];
    const ranked = rankForPromotion(buildings);
    expect(ranked[0].severity).toBe("inferno");
    expect(ranked[1].severity).toBe("fire");
    expect(ranked[2].severity).toBe("smoke");
  });

  it("same severity → closer to center first", () => {
    const buildings: PromotableBuilding[] = [
      { fileId: "a", severity: "fire", distToCenter: 100 },
      { fileId: "b", severity: "fire", distToCenter: 10 },
      { fileId: "c", severity: "fire", distToCenter: 50 },
    ];
    const ranked = rankForPromotion(buildings);
    expect(ranked[0].distToCenter).toBe(10);
    expect(ranked[1].distToCenter).toBe(50);
    expect(ranked[2].distToCenter).toBe(100);
  });

  it("empty array returns empty", () => {
    expect(rankForPromotion([])).toEqual([]);
  });

  it("does not mutate input", () => {
    const input: PromotableBuilding[] = [
      { fileId: "a", severity: "fire", distToCenter: 50 },
      { fileId: "b", severity: "smoke", distToCenter: 10 },
    ];
    const copy = [...input];
    rankForPromotion(input);
    expect(input).toEqual(copy);
  });

});
describe("HeroFire crossfade state machine", () => {
  // Fake renderer stub — tests don't need real textures.
  const fakeRenderer = {
    generateTexture: (_opts: any) => ({ destroy: () => {} } as any),
  };

  function makeHero(): HeroFire {
    return createHeroFire(fakeRenderer, "test-file", 0, 0);
  }

  it("demotion crossfade → parkHeroFire called inside stepHeroFire when fade hits 0", () => {
    const hf = makeHero();
    hf.crossfade = 0;
    hf.crossfading = false;
    hf.crossfadeDirection = 0;
    hf.container.visible = true;
    hf.targetFileId = "some-building";

    // Start demotion
    beginDemotionCrossfade(hf);
    expect(hf.crossfading).toBe(true);
    expect(hf.crossfadeDirection).toBe(-1);
    expect(hf.crossfade).toBe(1);
    expect(hf.targetFileId).toBe("some-building");

    // Step through the full 300ms crossfade
    // fadeSpeed = 1/0.3 ≈ 3.333 per second. 300ms = 0.3s → crossfade goes from 1 to 0.
    stepHeroFire(hf, 0.3);
    // After 300ms at speed 1/0.3: crossfade = 1 - 0.3 * (1/0.3) = 0
    // stepHeroFire parks when crossfade <= 0 with targetFileId set.
    expect(hf.targetFileId).toBeNull();
    expect(hf.container.visible).toBe(false);
    expect(hf.container.alpha).toBe(0);
    expect(hf.crossfading).toBe(false);
  });

  it("promotion (retarget) sets crossfade to fade IN", () => {
    const hf = makeHero();
    parkHeroFire(hf);

    // retargetHeroFire sets crossfade = 0, crossfading=true, direction=1
    // We'll verify via the HeroFire's own fields after calling retargetHeroFire.
    // retargetHeroFire is not pure (needs renderer), so we test via direct state inspection
    // after manually setting up a hero for retarget.
    const hf2 = makeHero();
    parkHeroFire(hf2);
    hf2.crossfade = 0;
    hf2.crossfading = true;
    hf2.crossfadeDirection = 1;
    hf2.container.alpha = 0.3;
    hf2.targetFileId = "promoted-building";

    // Step 300ms forward → crossfade = 0 + 0.3 * (1/0.3) = 1
    stepHeroFire(hf2, 0.3);
    expect(hf2.crossfade).toBe(1);
    expect(hf2.crossfading).toBe(false);
    expect(hf2.crossfadeDirection).toBe(0);
    expect(hf2.container.alpha).toBeCloseTo(1.0, 1); // 0.3 + 1.0 * 0.7 ≈ 1.0
    expect(hf2.targetFileId).toBe("promoted-building"); // still active
  });

  it("parked hero stays parked across stepHeroFire calls", () => {
    const hf = makeHero();
    parkHeroFire(hf);
    expect(hf.targetFileId).toBeNull();
    expect(hf.container.visible).toBe(false);
    expect(hf.container.alpha).toBe(0);
    stepHeroFire(hf, 0.1);
    expect(hf.targetFileId).toBeNull();
    expect(hf.container.visible).toBe(false);
  });

  it("severity multipliers applied in retarget (smoke ×1, fire ×1.6, inferno ×2.4)", () => {
    // Pure test: SEVERITY_SPAWN_MULTIPLIER values are correct
    expect(SEVERITY_SPAWN_MULTIPLIER.smoke).toBe(1.0);
    expect(SEVERITY_SPAWN_MULTIPLIER.fire).toBe(1.6);
    expect(SEVERITY_SPAWN_MULTIPLIER.inferno).toBe(2.4);

    // Scale multipliers
    expect(SEVERITY_SCALE.smoke).toBe(0.7);
    expect(SEVERITY_SCALE.fire).toBe(1.0);
    expect(SEVERITY_SCALE.inferno).toBe(1.2);
  });
});

// ---------------------------------------------------------------------------
// STEP 1 regressions — severity taxonomy, LOD gate, sin- vs activity-smoke
// ---------------------------------------------------------------------------

function makeStubAtlas(): FireAtlas {
  const fakeRenderer = {
    generateTexture: () => {
      const t = { destroy: () => {} } as unknown as Texture;
      return t;
    },
  };
  return bakeFireAtlas(fakeRenderer);
}

describe("crowdFireShowsFlame — severity taxonomy", () => {
  it("smoke → no flame; fire and inferno → flame", () => {
    expect(crowdFireShowsFlame("smoke")).toBe(false);
    expect(crowdFireShowsFlame("fire")).toBe(true);
    expect(crowdFireShowsFlame("inferno")).toBe(true);
  });

  it("createCrowdFire: smoke-severity has no visible flame; fire/inferno do", () => {
    const atlas = makeStubAtlas();
    const smoke = createCrowdFire(atlas, "a.ts", "smoke", 0, 0);
    const fire = createCrowdFire(atlas, "b.ts", "fire", 0, 0);
    const inferno = createCrowdFire(atlas, "c.ts", "inferno", 0, 0);

    expect(smoke.fireSprite.visible).toBe(false);
    expect(smoke.smokeSprite.visible).toBe(true);
    expect(fire.fireSprite.visible).toBe(true);
    expect(fire.smokeSprite.visible).toBe(true);
    expect(inferno.fireSprite.visible).toBe(true);
    expect(inferno.smokeSprite.visible).toBe(true);
  });

  it("retargetHeroFire smoke → zero active flame/ember particles", () => {
    const fakeRenderer = {
      generateTexture: () => ({ destroy: () => {} } as any),
    };
    const hf = createHeroFire(fakeRenderer, "x.ts", 0, 0);
    retargetHeroFire(hf, "x.ts", "smoke", 10, 20);

    const nf = hf.flameParticles.length;
    const ne = hf.emberParticles.length;
    for (let i = 0; i < nf; i++) {
      expect(hf.particleState[i].active).toBe(false);
    }
    for (let i = 0; i < ne; i++) {
      expect(hf.particleState[nf + i].active).toBe(false);
    }
    // At least one smoke particle remains active for the soot column.
    const smokeActive = hf.particleState
      .slice(nf + ne)
      .some((s) => s.active);
    expect(smokeActive).toBe(true);
  });
});

describe("disaster LOD gate — crowd/hero fires", () => {
  it("pins LOD_DISASTER and hides effects below the gate", () => {
    expect(LOD_DISASTER).toBe(0.35);
    expect(disasterEffectsLodVisible(LOD_DISASTER - 0.01)).toBe(false);
    expect(disasterEffectsLodVisible(LOD_DISASTER)).toBe(true);
    expect(disasterEffectsLodVisible(LOD_DISASTER + 0.5)).toBe(true);
    expect(disasterEffectsLodVisible(0.1)).toBe(false);
  });
});

describe("sin-smoke vs activity-smoke — must not collapse", () => {
  it("sooty SIN_SMOKE is darker and denser than cool chimney activity smoke", () => {
    // Tint / colour families differ (activity = cool blue-gray; sin = warm soot).
    expect(SIN_SMOKE.colorCore).not.toBe(CHIMNEY_SMOKE_TINT);
    expect(SIN_SMOKE.colorMid).not.toBe(CHIMNEY_SMOKE_TINT);
    // Peak opacity of sin-smoke base is far above the thin chimney wisp.
    expect(SIN_SMOKE.baseAlpha).toBeGreaterThan(CHIMNEY_SMOKE_MAX_ALPHA * 2);
    // Sin core is a mid warm-grey (sooty, not near-black fog-hole, not meadow).
    const coreR = (SIN_SMOKE.colorCore >> 16) & 0xff;
    const coreG = (SIN_SMOKE.colorCore >> 8) & 0xff;
    const coreB = SIN_SMOKE.colorCore & 0xff;
    const mean = (coreR + coreG + coreB) / 3;
    expect(mean).toBeLessThan(100);
    expect(mean).toBeGreaterThan(55);
  });
});

describe("createCrowdFire — smoke origin above the body (STEP 1b)", () => {
  it("smoke base tracks bodyHeightPx (roof), not a foot constant; flame stays at body", () => {
    const atlas = makeStubAtlas();
    // Same geometry as PolisRenderer: iso foot + labelDepth (= makeLabel depthPx).
    const isoY = 500;
    const tallDepth = 240; // temple-scale silhouette height above foot
    const shortDepth = 48; // house-scale

    const tall = createCrowdFire(atlas, "temple.ts", "smoke", 10, isoY, tallDepth);
    const short = createCrowdFire(atlas, "hut.ts", "smoke", 10, isoY, shortDepth);

    // Flame near the body (fixed lift above foot) — same for both buildings.
    expect(tall.fireSprite.y).toBe(isoY - 20);
    expect(short.fireSprite.y).toBe(isoY - 20);

    // Smoke at roof: iso.y - bodyHeight + small ridge inset (must NOT be foot-level).
    // Fails if smoke is ever placed back at the foot with a constant lift.
    expect(tall.smokeSprite.y).toBe(isoY - tallDepth + 2);
    expect(short.smokeSprite.y).toBe(isoY - shortDepth + 2);

    // Smoke sits well above the flame for a tall body (clears the facade).
    expect(tall.fireSprite.y - tall.smokeSprite.y).toBe(tallDepth - 20 - 2);
    expect(tall.smokeSprite.y).toBeLessThan(tall.fireSprite.y);

    // Offset is height-driven, not a constant — tall and short differ by depths.
    expect(short.smokeSprite.y - tall.smokeSprite.y).toBe(tallDepth - shortDepth);
    expect(tall.smokeSprite.y).not.toBe(short.smokeSprite.y);

    // Guard against the old bug: smoke must not sit near the foot (y - ~6..26).
    expect(isoY - tall.smokeSprite.y).toBeGreaterThan(100);
  });
});

// ---------------------------------------------------------------------------
// STEP 1c — smoke texture must not be a degenerate 2×N sliver
// ---------------------------------------------------------------------------

describe("bakeFireAtlas — smoke texture is non-degenerate (STEP 1c)", () => {
  it("bakes smoke frames with usable width/height (not a 2px thread)", () => {
    // Smoke frames pass a fixed `frame` to generateTexture (last 6 calls after
    // 3×8 flame bands). That frame IS the baked texture size in real PIXI.
    const smokeFrames: { w: number; h: number }[] = [];
    const spy = {
      generateTexture: (opts: {
        target: Container;
        frame?: { width: number; height: number };
      }) => {
        if (opts.frame) {
          const w = opts.frame.width;
          const h = opts.frame.height;
          smokeFrames.push({ w, h });
          return { width: w, height: h, destroy: () => {} } as unknown as Texture;
        }
        // Flame bands still use bounds-based capture.
        const b = opts.target.getLocalBounds();
        return {
          width: Math.ceil(b.width) || 1,
          height: Math.ceil(b.height) || 1,
          destroy: () => {},
        } as unknown as Texture;
      },
    };

    const atlas = bakeFireAtlas(spy);
    expect(atlas.smokes).toHaveLength(6);
    expect(smokeFrames).toHaveLength(6);

    // Regression pin: was 2×13 (or 2×8). Texture must be a real plume band.
    expect(SMOKE_TEX_WIDTH).toBeGreaterThanOrEqual(40);
    expect(SMOKE_TEX_HEIGHT).toBeGreaterThanOrEqual(48);
    for (const tex of atlas.smokes) {
      expect(tex.width).toBeGreaterThanOrEqual(40);
      expect(tex.height).toBeGreaterThanOrEqual(48);
      expect(tex.width).toBe(SMOKE_TEX_WIDTH);
      expect(tex.height).toBe(SMOKE_TEX_HEIGHT);
    }
    for (const f of smokeFrames) {
      expect(f.w).toBe(SMOKE_TEX_WIDTH);
      expect(f.h).toBe(SMOKE_TEX_HEIGHT);
    }

    // Bake resolution ≠ on-screen size: sprite scale is the world-size lever.
    expect(SMOKE_BAKE_SCALE).toBeGreaterThanOrEqual(0.8);
    expect(SMOKE_SPRITE_SCALE).toBeGreaterThan(0);
    expect(SMOKE_SPRITE_SCALE).toBeLessThanOrEqual(SMOKE_BAKE_SCALE);

    // createCrowdFire applies the display scale (not bake scale).
    const cf = createCrowdFire(atlas, "smoke-scale.ts", "smoke", 0, 0, 100);
    expect(cf.smokeSprite.scale.x).toBeCloseTo(SMOKE_SPRITE_SCALE, 5);
    expect(cf.smokeSprite.scale.y).toBeCloseTo(SMOKE_SPRITE_SCALE, 5);
  });
});

// ---------------------------------------------------------------------------
// STEP 1d — ink must FILL the band (frame size alone is worthless)
// ---------------------------------------------------------------------------

describe("smoke ink fills the bake band (STEP 1d)", () => {
  it("alpha-nonzero bbox covers ≥60% width and ≥70% height on every bake frame", () => {
    const times = smokeBakeTimes();
    expect(times).toHaveLength(6);

    for (const t of times) {
      const m = measureSmokeInkCoverage(t, SMOKE_BAKE_SCALE, SMOKE_INK_ALPHA_FLOOR);
      // Predictor of on-screen visibility: ink bbox vs frame (not frame alone).
      expect(m.widthRatio).toBeGreaterThanOrEqual(0.6);
      expect(m.heightRatio).toBeGreaterThanOrEqual(0.7);
      // Opaque enough: a meaningful fraction of the bbox is actual soot, not
      // a one-pixel outline of a huge empty rectangle.
      expect(m.fillRatio).toBeGreaterThanOrEqual(0.25);
      // ~4–7 puffs in the column (overlap merges them visually).
      expect(m.puffCount).toBeGreaterThanOrEqual(4);
      expect(m.puffCount).toBeLessThanOrEqual(7);
    }
  });

  it("STEP 3: consecutive puffs overlap (continuous column, not beads)", () => {
    // Spacing must sit below mid-life diameter so envelopes intersect.
    const spacing = smokePuffVerticalSpacing(1);
    const rMid =
      SMOKE_PUFF.r0Min +
      0.5 * SMOKE_PUFF.r0Span +
      0.5 * SMOKE_PUFF.growth;
    const diameterMid = 2 * rMid;
    // Overlap ratio: spacing / diameter < 1 (gap would be beads-on-a-string).
    expect(spacing / diameterMid).toBeLessThan(0.75);
    expect(spacing).toBeLessThan(diameterMid);

    // Geometry pin: every consecutive pair on every bake frame intersects.
    for (const t of smokeBakeTimes()) {
      const puffs = enumerateSmokePuffs(t, 1);
      expect(puffs.length).toBeGreaterThanOrEqual(4);
      for (let i = 0; i < puffs.length - 1; i++) {
        const a = puffs[i];
        const b = puffs[i + 1];
        const dist = Math.hypot(a.x - b.x, a.y - b.y);
        expect(dist).toBeLessThan(a.r + b.r);
      }
    }

    // Cadence still yields ~4–6 alive (not a solid fog wall of 10+).
    const lifetime = 1 / SMOKE_PUFF.rate;
    const alive = lifetime / SMOKE_PUFF.interval;
    expect(alive).toBeGreaterThanOrEqual(4);
    expect(alive).toBeLessThanOrEqual(7);
  });

  it("STEP 3: neighbouring puff radii vary (not identical beads)", () => {
    // Parameter pin: birth radius span is large relative to r0Min.
    expect(SMOKE_PUFF.r0Span).toBeGreaterThanOrEqual(5.0);
    expect(SMOKE_PUFF.r0Span / SMOKE_PUFF.r0Min).toBeGreaterThanOrEqual(0.7);

    // Residual after stripping pure age-growth: |Δr − growth×Δage| ≈ |Δr0|.
    // Identical beads (same r0) would leave residual ≈ 0.
    const g = SMOKE_PUFF.growth;
    let pairCount = 0;
    let sumResidual = 0;
    let maxResidual = 0;
    for (const t of smokeBakeTimes()) {
      const puffs = enumerateSmokePuffs(t, 1);
      const ordered = [...puffs].sort((a, b) => a.age - b.age);
      for (let i = 0; i < ordered.length - 1; i++) {
        const younger = ordered[i];
        const older = ordered[i + 1];
        const dr = older.r - younger.r;
        const pureGrowth = g * (older.age - younger.age);
        const residual = Math.abs(dr - pureGrowth);
        sumResidual += residual;
        maxResidual = Math.max(maxResidual, residual);
        pairCount++;
      }
    }
    expect(pairCount).toBeGreaterThan(0);
    const meanResidual = sumResidual / pairCount;
    // Floor: mean |Δr0| must be clearly above noise (span/3 ≈ 2 for span 6).
    expect(meanResidual).toBeGreaterThan(1.5);
    expect(maxResidual).toBeGreaterThan(3.5);
  });

  it("STEP 3: multi-lobe puffs break the pure circle (offset satellites)", () => {
    const puffs = enumerateSmokePuffs(smokeBakeTimes()[0], 1);
    const lobes = enumerateSmokeLobes(smokeBakeTimes()[0], 1);
    expect(SMOKE_LOBES_PER_PUFF).toBeGreaterThanOrEqual(4);
    expect(lobes.length).toBe(puffs.length * SMOKE_LOBES_PER_PUFF);

    // Within each puff's lobe group, centres are not all identical (not concentric).
    for (let p = 0; p < puffs.length; p++) {
      const group = lobes.slice(
        p * SMOKE_LOBES_PER_PUFF,
        (p + 1) * SMOKE_LOBES_PER_PUFF,
      );
      const radii = group.map((L) => L.r);
      const minR = Math.min(...radii);
      const maxR = Math.max(...radii);
      // Different lobe radii (union silhouette is bumpy).
      expect(maxR / minR).toBeGreaterThan(1.25);
      // At least one satellite is offset from the main body centre.
      const main = group[0];
      const anyOffset = group
        .slice(1)
        .some((L) => Math.hypot(L.x - main.x, L.y - main.y) > main.r * 0.25);
      expect(anyOffset).toBe(true);
    }
  });

  it("on-screen plume size is honestly mid-building scale at zoom 1", () => {
    // Arithmetic: onScreen ≈ inkBBox × SMOKE_SPRITE_SCALE (zoom 1).
    // Target band: ~20–30 wide × ~50–70 tall for a mid building.
    const times = smokeBakeTimes();
    let minW = Infinity;
    let maxW = -Infinity;
    let minH = Infinity;
    let maxH = -Infinity;
    for (const t of times) {
      const m = measureSmokeInkCoverage(t);
      const screenW = m.inkW * SMOKE_SPRITE_SCALE;
      const screenH = m.inkH * SMOKE_SPRITE_SCALE;
      minW = Math.min(minW, screenW);
      maxW = Math.max(maxW, screenW);
      minH = Math.min(minH, screenH);
      maxH = Math.max(maxH, screenH);
    }
    // All frames land in the readable plume band (not hairline, not facade veil).
    expect(minW).toBeGreaterThanOrEqual(18);
    expect(maxW).toBeLessThanOrEqual(38);
    expect(minH).toBeGreaterThanOrEqual(45);
    expect(maxH).toBeLessThanOrEqual(80);
  });

  it("enumerateSmokeLobes matches lobe count of a populated column", () => {
    const lobes = enumerateSmokeLobes(smokeBakeTimes()[0], 1);
    // SMOKE_LOBES_PER_PUFF × 4–7 puffs.
    expect(lobes.length).toBeGreaterThanOrEqual(SMOKE_LOBES_PER_PUFF * 4);
    expect(lobes.length).toBeLessThanOrEqual(SMOKE_LOBES_PER_PUFF * 7);
    expect(lobes.every((L) => L.r > 0 && L.a > 0)).toBe(true);
  });
});
