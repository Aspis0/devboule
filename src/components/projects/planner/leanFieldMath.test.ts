// Unit tests for the PURE Kairion lean-field math (node env, no React / no canvas).
// Covers the two load-bearing mappings the graphic is built on: unrest -> jitter
// (insecurity = instability) and pull -> position (the marker's gravitation).

import { describe, it, expect } from "vitest";
import {
  FIELD_PAD_X,
  JITTER_AMPLITUDE,
  clamp01,
  optionX,
  weightedCenterX,
  jitterOffset,
  markerRadius,
  easedToward,
  effectiveUnrest,
  leanIsSoft,
  leanLineAlpha,
  isSettled,
} from "./leanFieldMath";

describe("clamp01", () => {
  it("clamps into [0,1] and maps non-finite to 0", () => {
    expect(clamp01(-2)).toBe(0);
    expect(clamp01(0.5)).toBe(0.5);
    expect(clamp01(9)).toBe(1);
    expect(clamp01(NaN)).toBe(0); // non-finite => safe-calm 0
    expect(clamp01(Infinity)).toBe(0); // non-finite => 0 (the guard runs before the >1 clamp)
  });
});

describe("optionX (pull -> position layout)", () => {
  it("centers a single option", () => {
    expect(optionX(0, 1, 320)).toBe(160);
  });

  it("places the first/last options at the padded edges", () => {
    const w = 320;
    expect(optionX(0, 3, w)).toBe(FIELD_PAD_X); // 46
    expect(optionX(2, 3, w)).toBe(w - FIELD_PAD_X); // 274
  });

  it("evenly spaces the middle option", () => {
    expect(optionX(1, 3, 320)).toBe(160);
  });
});

describe("weightedCenterX (pull -> marker rest position)", () => {
  const xs = [46, 274]; // two options at the padded edges of a 320-wide field

  it("rests ON the option that holds all the pull", () => {
    expect(weightedCenterX(xs, [1, 0], 160)).toBe(46);
    expect(weightedCenterX(xs, [0, 1], 160)).toBe(274);
  });

  it("rests in the torn middle when the pull is balanced (genuinely split)", () => {
    expect(weightedCenterX(xs, [0.5, 0.5], 160)).toBe(160);
  });

  it("gravitates proportionally toward the stronger pull", () => {
    // 0.75 toward x=46, 0.25 toward x=274 => weighted mean 103
    expect(weightedCenterX(xs, [0.75, 0.25], 160)).toBeCloseTo(103, 6);
  });

  it("falls back to the field center when there is no pull at all", () => {
    expect(weightedCenterX(xs, [0, 0], 999)).toBe(999);
  });

  it("floors negative / non-finite weights to 0", () => {
    expect(weightedCenterX(xs, [-5, 1], 160)).toBe(274);
    expect(weightedCenterX(xs, [NaN, 1], 160)).toBe(274);
  });
});

describe("jitterOffset (unrest -> tremor)", () => {
  it("is dead still at zero unrest (no instability)", () => {
    for (const t of [0, 0.3, 1.1, 5.7]) {
      expect(jitterOffset(0, t, 3)).toBe(0);
    }
  });

  it("scales LINEARLY with unrest at a fixed time/seed", () => {
    const t = 0.42;
    const seed = 3;
    const full = jitterOffset(1, t, seed);
    expect(jitterOffset(0.5, t, seed)).toBeCloseTo(full * 0.5, 9);
    expect(jitterOffset(0.25, t, seed)).toBeCloseTo(full * 0.25, 9);
  });

  it("never exceeds the amplitude envelope (|wave| <= 1)", () => {
    for (const t of [0, 0.1, 0.37, 1.9, 4.4, 7.0]) {
      expect(Math.abs(jitterOffset(1, t, 2))).toBeLessThanOrEqual(JITTER_AMPLITUDE + 1e-9);
    }
  });

  it("treats out-of-range unrest as clamped", () => {
    const t = 0.42;
    expect(jitterOffset(5, t, 3)).toBeCloseTo(jitterOffset(1, t, 3), 9);
    expect(jitterOffset(-1, t, 3)).toBe(0);
  });
});

describe("markerRadius", () => {
  it("is a firm dot when resolved (no breathing)", () => {
    expect(markerRadius(0.9, 1.23, true)).toBe(6);
  });

  it("does not breathe at zero unrest", () => {
    expect(markerRadius(0, 1.23, false)).toBe(5);
  });

  it("breathes around 5 by unrest", () => {
    const r = markerRadius(1, 0.0, false); // sin(0)=0 -> exactly 5
    expect(r).toBeCloseTo(5, 9);
  });

  it("breathes to its peak at the top of the sine (5 + 1.2)", () => {
    // time*4 = π/2 -> sin = 1 -> 5 + 1.2*1 = 6.2 (full-unrest breathing peak)
    expect(markerRadius(1, Math.PI / 8, false)).toBeCloseTo(6.2);
  });
});

describe("easedToward", () => {
  it("moves a fraction of the way toward the target", () => {
    expect(easedToward(0, 1, 0.08)).toBeCloseTo(0.08, 9);
    expect(easedToward(10, 0, 0.5)).toBeCloseTo(5, 9);
  });

  it("is a no-op when already at the target", () => {
    expect(easedToward(3, 3, 0.3)).toBe(3);
  });
});

describe("effectiveUnrest", () => {
  it("passes an open doubt's unrest through (clamped)", () => {
    expect(effectiveUnrest(0.3, "open")).toBe(0.3);
    expect(effectiveUnrest(2, "open")).toBe(1);
  });

  it("guarantees a visible destabilisation on reopen", () => {
    expect(effectiveUnrest(0.1, "reopened")).toBe(0.6);
    expect(effectiveUnrest(0.9, "reopened")).toBe(0.9);
  });
});

describe("leanIsSoft / leanLineAlpha (honesty layer)", () => {
  it("marks a low-confidence lean as soft", () => {
    expect(leanIsSoft(0.2)).toBe(true);
    expect(leanIsSoft(0.49)).toBe(true);
    expect(leanIsSoft(0.5)).toBe(false);
    expect(leanIsSoft(0.9)).toBe(false);
  });

  it("dims the leaned tension line when confidence is low (hint, not verdict)", () => {
    const pull = 0.6;
    const firm = leanLineAlpha(pull, true, 0.9);
    const soft = leanLineAlpha(pull, true, 0.2);
    expect(soft).toBeLessThan(firm);
  });

  it("does not dim a non-lean line regardless of confidence", () => {
    const pull = 0.6;
    expect(leanLineAlpha(pull, false, 0.2)).toBe(leanLineAlpha(pull, false, 0.9));
  });

  it("brightens with pull", () => {
    expect(leanLineAlpha(1, false, 0.9)).toBeGreaterThan(leanLineAlpha(0, false, 0.9));
  });
});

describe("isSettled", () => {
  it("is settled only when unrest≈0 AND one candidate dominates", () => {
    expect(isSettled([{ label: "a", pull: 1 }, { label: "b", pull: 0 }], 0)).toBe(true);
    expect(isSettled([{ label: "a", pull: 0.5 }, { label: "b", pull: 0.5 }], 0)).toBe(false);
    expect(isSettled([{ label: "a", pull: 1 }, { label: "b", pull: 0 }], 0.5)).toBe(false);
  });

  it("is not settled with no candidates", () => {
    expect(isSettled([], 0)).toBe(false);
  });
});
