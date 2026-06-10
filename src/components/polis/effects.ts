// Effects — 30fps STEPPED ambient animation, pooled + allocation-free.
//
// RETRO CADENCE: nothing here uses smooth per-frame lerp. A StepClock advances
// an integer `animFrame` every ~33ms (1000/30). Every animation reads that
// frame and switches between a few DISCRETE states (modulo cycles) — the chunky,
// pre-rendered-sprite feel of Caesar III / Zeus.
//
// PERFORMANCE CONTRACT:
//   - All geometry is built ONCE (flame/flag frames in buildings.ts, water
//     patches here, smoke particles in a fixed reused POOL).
//   - The ticker NEVER calls Graphics.clear()+refill. It only mutates
//     transform / alpha / visibility / tint on pre-built objects, or repositions
//     a recycled particle from the pool.
//   - Only emitters/handles in CURRENTLY-VISIBLE chunks are animated; culled
//     ones are skipped (and their visible particles parked off-screen).

import { Graphics } from "pixi.js";

export const STEP_MS = 1000 / 30; // 30fps stepped clock

/** Integer-stepped animation clock. */
export class StepClock {
  private acc = 0;
  /** Monotonic integer frame; all stepped animations key off this. */
  frame = 0;

  /** Advance by elapsed ms; returns true if the frame changed this tick. */
  advance(deltaMs: number): boolean {
    this.acc += deltaMs;
    let changed = false;
    // Clamp catch-up so a long stall (tab backgrounded) can't spin forever.
    let budget = 4;
    while (this.acc >= STEP_MS && budget-- > 0) {
      this.acc -= STEP_MS;
      this.frame++;
      changed = true;
    }
    if (this.acc > STEP_MS) this.acc = 0; // drop the backlog
    return changed;
  }

  reset(): void {
    this.acc = 0;
    this.frame = 0;
  }
}

// ---------------------------------------------------------------------------
// Stepped state helpers — pick which pre-built frame is visible this step.
// All callers pass the SAME `frame`, so the whole city steps in lockstep.
// (Building ambient anim — smoke/flame/flag/water — now lives in the ported
// Claude Design kit, kitcd/anims.ts. These helpers remain for AgentLayer.)
// ---------------------------------------------------------------------------

/** Show exactly one frame from a set, cycling on the step clock. */
export function cycleFrames(
  frames: Graphics[],
  frame: number,
  every = 1,
  offset = 0,
): void {
  const idx = Math.floor((frame + offset) / every) % frames.length;
  for (let i = 0; i < frames.length; i++) frames[i].visible = i === idx;
}

/** Hide every frame in a set (used when a chunk is culled). */
export function hideFrames(frames: Graphics[]): void {
  for (const f of frames) f.visible = false;
}

/**
 * Stepped pulse value in {a, b, c} cycling on the clock — used for beacon /
 * glow alpha so even the "pulse" is chunky, not a continuous sine.
 */
export function steppedPulse(
  frame: number,
  levels: readonly number[],
  every = 2,
): number {
  const idx = Math.floor(frame / every) % levels.length;
  return levels[idx];
}
