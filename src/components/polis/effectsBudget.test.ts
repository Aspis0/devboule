import { describe, it, expect } from "vitest";
import { EffectsBudget, budgetAllowanceMs } from "./effectsBudget";
import type { RenderProfile } from "./renderProfile";

const RICH_PROFILE: RenderProfile = {
  tier: "rich",
  lodLabelsIn: 0.62,
  lodLabelsOut: 0.58,
  lodDetails: 0.4,
  lodAgents: 0.35,
  preloadRing: 2,
  atlasResolutionCap: 2,
  maxAmbientWalkers: 40,
  antialias: true,
  maxHeroFires: 6,
};
const LEAN_PROFILE: RenderProfile = { ...RICH_PROFILE, tier: "lean", maxHeroFires: 3 };
const MINIMAL_PROFILE: RenderProfile = { ...RICH_PROFILE, tier: "minimal", maxHeroFires: 0 };

describe("budgetAllowanceMs", () => {
  it("RICH → 3.0ms", () => expect(budgetAllowanceMs(RICH_PROFILE)).toBe(3.0));
  it("LEAN → 2.0ms", () => expect(budgetAllowanceMs(LEAN_PROFILE)).toBe(2.0));
  it("MINIMAL → 1.0ms", () => expect(budgetAllowanceMs(MINIMAL_PROFILE)).toBe(1.0));
});

describe("EffectsBudget — EMA smoothing", () => {
  it("first tick sets smoothedCostMs to raw value", () => {
    let t = 0;
    const b = new EffectsBudget(RICH_PROFILE, () => t);
    b.record(4.0);
    expect(b.smoothedCostMs).toBe(4.0);
  });

  it("EMA α=0.2 smooths subsequent ticks", () => {
    let t = 0;
    const b = new EffectsBudget(RICH_PROFILE, () => t);
    b.record(4.0);
    t = 1;
    b.record(1.0);
    expect(b.smoothedCostMs).toBeCloseTo(3.4, 5);
    t = 2;
    b.record(1.0);
    expect(b.smoothedCostMs).toBeCloseTo(2.92, 5);
  });
});

describe("EffectsBudget — demotion ladder", () => {
  it("first tick does not advance", () => {
    let t = 100;
    const b = new EffectsBudget(RICH_PROFILE, () => t);
    b.record(5.0);
    expect(b.rung).toBe(0);
  });

  it("over budget for 1 second of steady ticks → rung +1", () => {
    let t = 0;
    const b = new EffectsBudget(RICH_PROFILE, () => t);
    b.record(5.0); // first
    // 30 ticks at ~33ms, each over budget
    for (let i = 0; i < 30; i++) {
      t += 1 / 30;
      b.record(5.0);
    }
    expect(b.rung).toBe(0);
    t += 1 / 30;
    b.record(5.0); // 31st tick → crosses 1s → +1 rung
    expect(b.rung).toBe(1);
  });

  it("over budget for 2s → rung +2", () => {
    let t = 0;
    const b = new EffectsBudget(RICH_PROFILE, () => t);
    b.record(5.0);
    for (let i = 0; i < 62; i++) {
      t += 1 / 30;
      b.record(5.0);
    }
    expect(b.rung).toBe(2);
  });

  it("caps at rung 5", () => {
    let t = 0;
    const b = new EffectsBudget(RICH_PROFILE, () => t);
    b.record(5.0);
    // ~200 ticks = ~6.6s → should hit rung 5
    for (let i = 0; i < 200; i++) {
      t += 1 / 30;
      b.record(5.0);
    }
    expect(b.rung).toBe(5);
  });

  it("sustained low cost arrests demotion after EMA decay", () => {
    let t = 0;
    const b = new EffectsBudget(RICH_PROFILE, () => t);
    // 0.6s over → not enough yet
    b.record(5.0);
    for (let i = 0; i < 18; i++) { t += 1 / 30; b.record(5.0); }
    expect(b.rung).toBe(0);

    // Now low cost (1.0). EMA stays above allowance for a few ticks,
    // accumulating remaining ~0.4s → 1 demotion.
    for (let i = 0; i < 30; i++) { t += 1 / 30; b.record(1.0); }
    expect(b.rung).toBeLessThanOrEqual(1);

    // Sustained low cost → EMA drops below allowance → no further demotion
    for (let i = 0; i < 90; i++) { t += 1 / 30; b.record(0.5); }
    expect(b.rung).toBeLessThanOrEqual(1);
  });
});

describe("EffectsBudget — promotion hysteresis", () => {
  it("sustained under-budget at <66% → promote after 60 ticks + 2s", () => {
    let t = 0;
    const b = new EffectsBudget(RICH_PROFILE, () => t);
    // Demote to rung 2
    b.record(5.0);
    for (let i = 0; i < 62; i++) { t += 1 / 30; b.record(5.0); }
    expect(b.rung).toBe(2);

    // Sustained 0.5ms cost (well under 66% of 3.0)
    // EMA decays to 0.5 in ~15 ticks. Then need 60 consecutive + 2s promo ≈ 120 more.
    for (let i = 0; i < 160; i++) { t += 1 / 30; b.record(0.5); }
    expect(b.rung).toBeLessThanOrEqual(1);
  });

  it("an over-budget burst resets promotion and causes further demotion", () => {
    let t = 0;
    const b = new EffectsBudget(RICH_PROFILE, () => t);
    // Demote to rung 2
    b.record(5.0);
    for (let i = 0; i < 62; i++) { t += 1 / 30; b.record(5.0); }
    expect(b.rung).toBe(2);

    // Let EMA decay
    for (let i = 0; i < 40; i++) { t += 1 / 30; b.record(0.5); }

    // Now a burst of over-budget ticks: 3s worth
    for (let i = 0; i < 90; i++) { t += 1 / 30; b.record(8.0); }
    // 3s over → +3 rungs → rung 5
    expect(b.rung).toBeGreaterThanOrEqual(4);

    // Let EMA decay, then sustained under-budget for promotion
    for (let i = 0; i < 300; i++) { t += 1 / 30; b.record(0.5); }
    // Should have promoted back at least once
    expect(b.rung).toBeLessThanOrEqual(4);
  });

  it("smoothed cost above 66% threshold blocks promotion", () => {
    let t = 0;
    const b = new EffectsBudget(RICH_PROFILE, () => t);
    b.record(5.0);
    for (let i = 0; i < 62; i++) { t += 1 / 30; b.record(5.0); }
    const rungBefore = b.rung;

    // 2.4ms is under 3.0 allowance but above 66% (2.0)
    // This should NOT cause further demotion (under allowance)
    // but also NOT allow promotion (above 66% threshold)
    for (let i = 0; i < 200; i++) { t += 1 / 30; b.record(2.4); }
    expect(b.rung).toBe(rungBefore);
  });
});

  it("coasting at 90% allowance does NOT count toward promotion — must be <66%", () => {
    let t = 0;
    const b = new EffectsBudget(RICH_PROFILE, () => t);
    // Demote to rung 2
    b.record(5.0);
    for (let i = 0; i < 62; i++) { t += 1 / 30; b.record(5.0); }
    expect(b.rung).toBe(2);

    // 200 ticks at 2.7ms (90% of 3.0 allowance) — under allowance but ABOVE 66% (2.0).
    // consecutiveUnder should stay at 0 because we're above threshold.
    for (let i = 0; i < 200; i++) { t += 1 / 30; b.record(2.7); }
    expect(b.rung).toBe(2); // no promotion — never under 66%

    // Now dip under 66% threshold: EMA needs ~2 ticks to drop below 2.0,
    // then 60 consecutive ticks, then ~2s promo accumulation = ~122+ ticks.
    for (let i = 0; i < 130; i++) { t += 1 / 30; b.record(0.5); }
    expect(b.rung).toBe(1); // promoted after EMA decay + 60 consecutive + 2s promo
  });


describe("EffectsBudget — ladder order pinned", () => {
  it("never below 0", () => {
    let t = 0;
    const b = new EffectsBudget(RICH_PROFILE, () => t);
    for (let i = 0; i < 300; i++) { t += 1 / 30; b.record(0.1); }
    expect(b.rung).toBe(0);
  });

  it("never above 5", () => {
    let t = 0;
    const b = new EffectsBudget(RICH_PROFILE, () => t);
    b.record(10.0);
    for (let i = 0; i < 400; i++) { t += 1 / 30; b.record(10.0); }
    expect(b.rung).toBe(5);
  });

  it("reset returns to rung 0", () => {
    let t = 0;
    const b = new EffectsBudget(RICH_PROFILE, () => t);
    b.record(5.0);
    for (let i = 0; i < 200; i++) { t += 1 / 30; b.record(5.0); }
    expect(b.rung).toBe(5);
    b.reset();
    expect(b.rung).toBe(0);
    expect(b.smoothedCostMs).toBe(0);
  });
});
