// effectsBudget.ts — P5.1 measured effects-pass budget accumulator + demotion ladder.
//
// PURE — no PIXI, no DOM, no side effects. The `now()` clock is injectable (the
// renderer passes `performance.now` in prod and a mock in tests). Exported for
// headless vitest.
//
// The accumulator brackets the renderer's per-tick effects pass:
//   - measure: bracket with `performance.now()` around effects (fires, halos,
//     walkers' anims), call `record(elapsedMs)`.
//   - ladder: a rung 0–5 that gates which effects are enabled, demoted one rung
//     per second when over budget, promoted only after 60 consecutive ticks under
//     66% allowance and then one rung per 2 seconds (hysteresis — prevents
//     flapping).
//
// Allowance (ms per 33ms tick, ~9%/6%/3% of the frame):
//   RICH   3.0 ms
//   LEAN   2.0 ms
//   MINIMAL 1.0 ms

import type { RenderProfile } from "./renderProfile";

/** Rung 0 = full effects; rung 5 = everything off. The renderer reads this value
 *  and gates effect passes. Order is fixed — highest-cost effects disable first. */
export type BudgetRung = 0 | 1 | 2 | 3 | 4 | 5;

/** Rung labels for the debug overlay + tests. */
export const BUDGET_RUNG_LABELS: Readonly<Record<BudgetRung, string>> = {
  0: "full",
  1: "hero→crowd",
  2: "halo flicker freeze",
  3: "crowd 15fps",
  4: "ambient half-rate",
  5: "ambient pause",
} as const;

const MAX_RUNG: BudgetRung = 5;

/** Budget allowance per profile tier (ms). */
export function budgetAllowanceMs(profile: RenderProfile): number {
  if (profile.tier === "rich") return 3.0;
  if (profile.tier === "lean") return 2.0;
  return 1.0; // minimal
}

/** EMA smoothing factor for the rolling effects cost. */
const EMA_ALPHA = 0.2;

/** Promotion: require this many consecutive ticks under the threshold before
 *  the promotion countdown even starts. */
const PROMO_CONSECUTIVE_TICKS = 60;
/** Promotion threshold: smoothed cost must be under this fraction of allowance. */
const PROMO_THRESHOLD = 2 / 3; // 66%
/** Promotion pace: one rung per this many seconds. */
const PROMO_INTERVAL_S = 2.0;

/** Demotion pace: one rung per second while over budget. */
const DEMO_INTERVAL_S = 1.0;

/**
 * Pure effects-budget accumulator. The renderer owns ONE instance, brackets its
 * effects pass, and reads `.rung` (0–5) to gate expensive effects.
 *
 * Deterministic clock: `now()` is injectable so headless tests control time.
 */
export class EffectsBudget {
  /** Current ladder rung. 0 = all effects on; 5 = all off. */
  rung: BudgetRung = 0;

  /** The per-tick allowance (ms), set once from the profile. */
  readonly allowanceMs: number;

  /** Exponentially smoothed effects-pass cost (ms). */
  smoothedCostMs = 0;

  // --- internal ---
  /** Injectable monotonic-clock function (seconds since epoch, high precision). */
  private now: () => number;

  // Demotion state: accumulate time over budget.
  private overBudgetAccS = 0;
  private lastDemotionCheckS = 0;
  private demotionCheckInitialized = false;

  // Promotion state: consecutive ticks under threshold + promotion timer.
  private consecutiveUnder = 0;
  private promoAccS = 0;

  constructor(profile: RenderProfile, now: () => number) {
    this.allowanceMs = budgetAllowanceMs(profile);
    this.now = now;
  }

  /**
   * Record the elapsed ms of the effects pass for THIS tick. Called once per tick
   * immediately after the effects pass completes. Advances the ladder.
   */
  record(elapsedMs: number): void {
    // EMA smoothing.
    if (this.smoothedCostMs === 0) {
      this.smoothedCostMs = elapsedMs;
    } else {
      this.smoothedCostMs =
        EMA_ALPHA * elapsedMs + (1 - EMA_ALPHA) * this.smoothedCostMs;
    }
    this.advanceLadder();
  }

  /** Reset all state (scene rebuild / new city). */
  reset(): void {
    this.rung = 0;
    this.smoothedCostMs = 0;
    this.overBudgetAccS = 0;
    this.consecutiveUnder = 0;
    this.promoAccS = 0;
    this.demotionCheckInitialized = false;
  }

  // ---- internal ladder ----

  private advanceLadder(): void {
    const nowS = this.now();
    const over = this.smoothedCostMs > this.allowanceMs;

    if (!this.demotionCheckInitialized) {
      this.lastDemotionCheckS = nowS;
      this.demotionCheckInitialized = true;
      return; // first tick: no delta, can't advance yet
    }

    const deltaS = Math.max(0, nowS - this.lastDemotionCheckS);
    this.lastDemotionCheckS = nowS;

    if (over) {
      // Demotion: accumulate time over budget; move one rung per second.
      this.overBudgetAccS += deltaS;
      // Reset promotion state when over budget.
      this.consecutiveUnder = 0;
      this.promoAccS = 0;

      while (this.overBudgetAccS >= DEMO_INTERVAL_S && this.rung < MAX_RUNG) {
        this.overBudgetAccS -= DEMO_INTERVAL_S;
        this.rung = (this.rung + 1) as BudgetRung;
      }
    } else {
      // Under budget: reset demotion accumulator.
      this.overBudgetAccS = 0;

      // Promotion hysteresis: only count consecutive ticks when smoothed cost is
      // BELOW 66% of allowance (not just under 100%). Between 66% and 100% we
      // reset the counter — that's "coasting", not "ready to promote".
      if (this.smoothedCostMs < this.allowanceMs * PROMO_THRESHOLD) {
        this.consecutiveUnder++;
      } else {
        this.consecutiveUnder = 0;
      }

      if (
        this.consecutiveUnder >= PROMO_CONSECUTIVE_TICKS &&
        this.smoothedCostMs < this.allowanceMs * PROMO_THRESHOLD
      ) {
        // Accumulate promotion time. One rung per 2s.
        this.promoAccS += deltaS;
        while (this.promoAccS >= PROMO_INTERVAL_S && this.rung > 0) {
          this.promoAccS -= PROMO_INTERVAL_S;
          this.rung = (this.rung - 1) as BudgetRung;
        }
      }
    }
  }
}
