// leanFieldMath.ts — the PURE math behind the Kairion lean-field (insecurity =
// instability). Framework-free + DOM-free on purpose: LeanField.tsx imports these
// and feeds them a 2D canvas; this module holds zero React / zero canvas so the
// tremor/pull/position math is unit-testable in the repo's node vitest env.
//
// Ported VERBATIM (math-for-math) from `kairion/kairion.html` drawField():
//   const ox = i => n===1 ? w/2 : padX + span*(i/(n-1));
//   baseX = Σ(pull·ox) / Σpull;
//   jitter = sin(t*9.1+seed)*0.6 + sin(t*5.3+1)*0.4;
//   mx = baseX + jitter * unrest * 26;
//   r  = resolved ? 6 : 5 + sin(t*4)*1.2*unrest;
// The constants are kept exactly so the motion matches the mockup.

import type { DoubtCandidate } from "../../agents/agentConsoleModel";

/** Horizontal padding (px) inside the field before the first / after the last tick. */
export const FIELD_PAD_X = 46;
/** Max tremor amplitude (px) at unrest = 1. */
export const JITTER_AMPLITUDE = 26;

/** Clamp a number into [0, 1] (NaN -> 0). The signal is nominally 0..1 but the wire
 *  is untrusted, so every consumer funnels through this. */
export function clamp01(v: number): number {
  if (!Number.isFinite(v)) return 0;
  if (v < 0) return 0;
  if (v > 1) return 1;
  return v;
}

/** The x of option `i` of `n`, evenly spaced between the padded edges of a `width`-wide
 *  field. A single option sits dead-center. Pure + total. */
export function optionX(
  i: number,
  n: number,
  width: number,
  padX: number = FIELD_PAD_X,
): number {
  if (n <= 1) return width / 2;
  const span = width - padX * 2;
  return padX + span * (i / (n - 1));
}

/** The pull-weighted center of mass over the option positions — where the marker
 *  rests before tremor. `weights` are the candidate pulls (negatives floored to 0).
 *  When every weight is 0 (no signal / genuinely split with empty pulls) it falls back
 *  to the field center. `xs` and `weights` are zipped by index. Pure + total. */
export function weightedCenterX(
  xs: number[],
  weights: number[],
  fallback: number,
): number {
  let acc = 0;
  let sum = 0;
  const n = Math.min(xs.length, weights.length);
  for (let i = 0; i < n; i++) {
    const w = Math.max(0, Number.isFinite(weights[i]) ? weights[i] : 0);
    acc += w * xs[i];
    sum += w;
  }
  return sum > 0 ? acc / sum : fallback;
}

/** The tremor offset (px) added to the resting center at a given `time` (seconds).
 *  Amplitude scales LINEARLY with `unrest`: 0 unrest => 0 offset (dead still), 1 unrest
 *  => up to ±JITTER_AMPLITUDE. `seed` decorrelates concurrent doubts (the mock uses
 *  `q.id.length`). This is the heart of "insecurity = instability". Pure + total. */
export function jitterOffset(unrest: number, time: number, seed: number): number {
  const u = clamp01(unrest);
  const wave = Math.sin(time * 9.1 + seed) * 0.6 + Math.sin(time * 5.3 + 1) * 0.4;
  const r = wave * u * JITTER_AMPLITUDE;
  return r === 0 ? 0 : r; // normalize -0 (negative wave * 0 unrest) to +0

}

/** The marker radius (px): a resolved/still marker is a firm dot; an unsure one
 *  breathes by `unrest`. Pure + total. */
export function markerRadius(unrest: number, time: number, resolved: boolean): number {
  if (resolved) return 6;
  return 5 + Math.sin(time * 4) * 1.2 * clamp01(unrest);
}

/** One step of critically-damped easing of `cur` toward `target` at `rate` (0..1).
 *  Used to glide the per-candidate pulls + the unrest toward their live values each
 *  frame instead of snapping. Pure + total. */
export function easedToward(cur: number, target: number, rate: number): number {
  return cur + (target - cur) * rate;
}

/** The effective unrest the field should render: a `reopened` doubt is guaranteed a
 *  visible destabilisation even if the raw signal under-reports it (the orchestrator
 *  just changed its own mind). Otherwise the raw, clamped unrest. Pure + total. */
export function effectiveUnrest(unrest: number, status: "open" | "reopened"): number {
  const u = clamp01(unrest);
  return status === "reopened" ? Math.max(u, 0.6) : u;
}

/** Whether the lean is a SOFT signal (a hint, not a verdict): true when the orchestrator
 *  is not confident in its direction. Drives the honesty layer — at low confidence the
 *  lean line dims and the copy hedges; the tremor stays either way. Pure + total. */
export function leanIsSoft(directionConfidence: number): boolean {
  return clamp01(directionConfidence) < 0.5;
}

/** The alpha (0..1) of an option's tension line: brighter the more the marker is
 *  pulled toward it, but the LEANED line is held back when confidence is low so a
 *  shaky lean never reads as a firm commitment. Pure + total. */
export function leanLineAlpha(
  pull: number,
  isLeanLine: boolean,
  directionConfidence: number,
): number {
  const base = 0.12 + clamp01(pull) * 0.5;
  if (isLeanLine && leanIsSoft(directionConfidence)) {
    // halve toward the floor — a soft lean glows, but does not assert.
    return 0.12 + (base - 0.12) * 0.5;
  }
  return base;
}

/** A resolved doubt has snapped still: no instability left. In the frozen wire there
 *  is no "resolved" status (only open|reopened); a doubt is resolved by leaving the open
 *  set. The field treats unrest≈0 with a single dominant candidate as "settled" so a
 *  lingering card snaps rather than trembles. Pure + total. */
export function isSettled(candidates: DoubtCandidate[], unrest: number): boolean {
  if (clamp01(unrest) > 0.04) return false;
  const top = Math.max(0, ...candidates.map((c) => clamp01(c.pull)));
  return top >= 0.95;
}
