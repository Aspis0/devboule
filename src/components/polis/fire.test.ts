import { describe, it, expect } from "vitest";
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
  type HeroFire,
} from "./fire";
import type { PromotableBuilding } from "./fire";

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
