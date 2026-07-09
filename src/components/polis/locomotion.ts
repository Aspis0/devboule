// locomotion.ts — P5.2 pure citizen locomotion helpers: Catmull-Rom spline
// easing, lane-offset computation, per-building entry-slot allocator.
//
// PURE — no PIXI, no DOM, no side effects. Exported for headless vitest.
// Allocation-free ticker rule: fixed-size structures at init, no per-tick allocs.
//
// DETERMINISM: NO Math.random anywhere. Lane offset uses the existing
// deterministic hash (hashString from ./rng). Slot choice is by arrival order.

import { isoToCart } from "./iso";
import { roundTile } from "./navWalkable";
import { hashString } from "./rng";

// ---- IsoPoint (same shape as in iso.ts / AgentLayer) ----
export interface IPoint {
  x: number;
  y: number;
}

// =========================================================================
// 1. CATMULL-ROM SPLINE
// =========================================================================

/**
 * Sample a Catmull-Rom spline at parameter t ∈ [0, 1] through the four control
 * points p0, p1, p2, p3. The curve passes through p1 at t=0 and p2 at t=1.
 *
 * Standard Catmull-Rom tangent: 0.5 * (p2 - p0) at t=0, 0.5 * (p3 - p1) at t=1.
 * This is a handful of multiplies per call — suitable for per-walker per-tick.
 */
export function catmullRomPoint(
  p0: IPoint, p1: IPoint, p2: IPoint, p3: IPoint,
  t: number,
): IPoint {
  const t2 = t * t;
  const t3 = t2 * t;
  // Matrix form:
  //   x = 0.5 * ((2*p1) + (-p0 + p2)*t + (2*p0 - 5*p1 + 4*p2 - p3)*t^2 + (-p0 + 3*p1 - 3*p2 + p3)*t^3)
  const x =
    0.5 *
    (2 * p1.x +
      (-p0.x + p2.x) * t +
      (2 * p0.x - 5 * p1.x + 4 * p2.x - p3.x) * t2 +
      (-p0.x + 3 * p1.x - 3 * p2.x + p3.x) * t3);
  const y =
    0.5 *
    (2 * p1.y +
      (-p0.y + p2.y) * t +
      (2 * p0.y - 5 * p1.y + 4 * p2.y - p3.y) * t2 +
      (-p0.y + 3 * p1.y - 3 * p2.y + p3.y) * t3);
  return { x, y };
}

/**
 * Build a spline sampler for a single leg of a multi-segment polyline route.
 *
 * `waypoints` is the full route; `legIndex` selects the current segment
 * [legIndex, legIndex+1]. The 4-point window for Catmull-Rom is clamped at route
 * ends by repeating the endpoint (standard boundary condition — makes the first
 * and last segments still smooth into their endpoints).
 *
 * Returns a function `sample(t: number): IPoint` where t ∈ [0,1] maps along this
 * leg. The window is PRE-COMPUTED here (captured once per leg change), so the
 * per-tick call is just the ~30 arithmetic ops of catmullRomPoint.
 */
export function buildSplineLeg(
  waypoints: IPoint[],
  legIndex: number,
): (t: number) => IPoint {
  const n = waypoints.length;
  if (n < 2) {
    // Degenerate: just return the single point.
    const pt = waypoints[0] ?? { x: 0, y: 0 };
    return () => pt;
  }

  // Clamp legIndex.
  const li = Math.max(0, Math.min(legIndex, n - 2));

  // For 2-point routes, Catmull-Rom with repeated endpoints bows instead of
  // staying straight (p0=p1, p3=p2 causes an S-curve). Degrade to simple
  // linear interpolation — exact straight path, matches the spec requirement.
  if (n === 2) {
    const pa = waypoints[0];
    const pb = waypoints[1];
    return (t: number) => ({
      x: pa.x + (pb.x - pa.x) * Math.max(0, Math.min(1, t)),
      y: pa.y + (pb.y - pa.y) * Math.max(0, Math.min(1, t)),
    });
  }

  // Build the 4-point window with endpoint repetition for boundaries.
  const p1 = waypoints[li];
  const p2 = waypoints[li + 1];
  const p0 = li > 0 ? waypoints[li - 1] : p1; // repeat start
  const p3 = li < n - 2 ? waypoints[li + 2] : p2; // repeat end

  return (t: number) => catmullRomPoint(p0, p1, p2, p3, Math.max(0, Math.min(1, t)));
}

/**
 * A safe spline leg result: the effective sample function and metadata about
 * which safety mode was chosen. Returned by {@link buildSafeSplineLeg}.
 */
export interface SafeSplineLeg {
  /** "spline" when the Catmull-Rom curve is safe; "linear" when degraded. */
  mode: "spline" | "linear";
  /** Sample(t) for this leg — either the spline or the linear fallback. */
  sample: (t: number) => IPoint;
  /** True when the lane offset is forced to 0 for this leg because the
   *  extreme offset (\u00b1maxOffset px) landed on a blocked tile. */
  laneOffsetClamped: boolean;
}

/** Default extreme lane offset px to test. Matches the max from {@link laneOffset}.
 *  Exported so tests can reference it. */
export const MAX_LANE_OFFSET_PX = 4;

/**
 * Build a SAFE spline leg: if ANY sample of the raw Catmull-Rom spline lands
 * on a blocked tile, degrade THAT LEG to plain linear interpolation. If the
 * extreme lane offset (\u00b1maxOffset px) lands on a blocked tile, clamp the lane
 * offset to 0 for the whole leg.
 *
 * Validation runs ONCE at leg-build time (not per frame). The sample density
 * matches the walk stepping: ceil(segLen / 8) samples per leg (8px \u2248 half a
 * tile width, catching any blocked-tile crossing).
 *
 * Returns the effective sample function, the chosen mode, and whether the lane
 * offset is clamped.
 */
export function buildSafeSplineLeg(
  waypoints: IPoint[],
  legIndex: number,
  blocked: (gx: number, gy: number) => boolean,
  maxOffsetPx: number = MAX_LANE_OFFSET_PX,
): SafeSplineLeg {
  // Build the raw spline leg (Catmull-Rom or linear for 2-point routes).
  const splineFn = buildSplineLeg(waypoints, legIndex);
  const n = waypoints.length;
  if (n < 2) {
    return { mode: "spline", sample: splineFn, laneOffsetClamped: false };
  }
  const li = Math.max(0, Math.min(legIndex, n - 2));
  const a = waypoints[li];
  const b = waypoints[li + 1];
  const segLen = Math.hypot(b.x - a.x, b.y - a.y) || 1;
  const dx = b.x - a.x;
  const dy = b.y - a.y;

  // Number of samples: enough to catch a blocked tile crossing. 8px \u2248 half
  // a tile width (a tile is ~16px across in iso). At least 2 samples.
  const sampleCount = Math.max(2, Math.ceil(segLen / 8));

  // 1. Check if the raw spline samples are all safe.
  // T2 FIX: the spline returns ISO pixel coords; the blocker expects cartesian
  // tile-grid indices. Convert via isoToCart → roundTile before querying.
  let splineBlocked = false;
  for (let i = 0; i <= sampleCount; i++) {
    const t = i / sampleCount;
    const pt = splineFn(t);
    const cart = isoToCart(pt.x, pt.y);
    if (blocked(roundTile(cart.x), roundTile(cart.y))) {
      splineBlocked = true;
      break;
    }
  }

  // 2. If spline is blocked, degrade to linear for this leg.
  const mode: "spline" | "linear" = splineBlocked ? "linear" : "spline";
  const sample = splineBlocked
    ? (t: number) => {
        const clamped = Math.max(0, Math.min(1, t));
        return {
          x: a.x + (b.x - a.x) * clamped,
          y: a.y + (b.y - a.y) * clamped,
        };
      }
    : splineFn;

  // 3. Lane-offset clamping: test the extreme offset at each sample. If ANY
  //    offset sample lands on a blocked tile, force lane offset 0.
  // T2 FIX: perpendicular offsets are in ISO pixel space; convert to tile-grid
  // via isoToCart → roundTile before querying the blocker.
  let laneOffsetClamped = false;
  if (maxOffsetPx > 0) {
    for (let i = 0; i <= sampleCount; i++) {
      const t = i / sampleCount;
      const rawPt = sample(t);
      // Test both positive and negative extremes.
      const offPos = applyPerpendicularOffset(rawPt, dx, dy, maxOffsetPx);
      const offNeg = applyPerpendicularOffset(rawPt, dx, dy, -maxOffsetPx);
      const cartPos = isoToCart(offPos.x, offPos.y);
      const cartNeg = isoToCart(offNeg.x, offNeg.y);
      if (blocked(roundTile(cartPos.x), roundTile(cartPos.y)) || blocked(roundTile(cartNeg.x), roundTile(cartNeg.y))) {
        laneOffsetClamped = true;
        break;
      }
    }
  }

  return { mode, sample, laneOffsetClamped };
}

// =========================================================================
// 2. LANE OFFSET
// =========================================================================

/**
 * Fixed perpendicular lane offset for a walker on a shared road segment.
 * Deterministic: `hash(walkerId) % 9 - 4` px.
 *
 * Opposite travel directions bias to opposite signs: if `travelRightward` is
 * true (the walker's net horizontal direction along the segment is rightward),
 * the sign is left as computed; if false (leftward), the sign is flipped.
 * This makes opposing traffic separate without collision detection.
 */
export function laneOffset(walkerId: string): number {
  const h = hashString(walkerId);
  // Map hash to [-4, 4] integer px.
  return (Math.abs(h) % 9) - 4;
}

/**
 * Apply direction-aware lane offset. The raw offset from laneOffset() is
 * perpendicular to the road segment. `travelRightward` determines whether
 * the sign matches or flips so opposing traffic separates.
 */
/**
 * Direction-aware lane offset. `dirDx`, `dirDy` is the travel direction vector.
 * The sign is determined by the DOMINANT axis (larger magnitude) so opposite
 * traffic on the same road segment reliably separates: walkers moving in the
 * +X (or +Y) direction get the raw offset, -X (or -Y) get the flipped sign.
 */
export function directedLaneOffset(
  walkerId: string,
  dirDx: number,
  dirDy: number,
): number {
  const raw = laneOffset(walkerId);
  // Dominant axis: use the larger component to determine sign.
  const dom = Math.abs(dirDx) >= Math.abs(dirDy) ? dirDx : dirDy;
  return dom >= 0 ? raw : -raw;
}

/**
 * Apply a perpendicular offset to a point given a segment direction.
 * dx, dy is the segment's unit or near-unit direction vector.
 * offsetPx is the signed perpendicular offset in pixels.
 * Returns a new point offset perpendicular to the direction.
 */
export function applyPerpendicularOffset(
  pos: IPoint,
  dx: number,
  dy: number,
  offsetPx: number,
): IPoint {
  // Perpendicular to (dx, dy) is (-dy, dx), normalized.
  const len = Math.hypot(dx, dy) || 1;
  const px = -dy / len * offsetPx;
  const py = dx / len * offsetPx;
  return { x: pos.x + px, y: pos.y + py };
}

// =========================================================================
// 3. ENTRY-SLOT ALLOCATOR
// =========================================================================

/**
 * Per-building entry-slot allocator. Presentation state (NOT CityState).
 * 3 slots per building; arriving walkers take the lowest free slot.
 *
 * Slots are freed on departure/possession-release. A sweep on walker despawn
 * prevents leaked ids. Fourth-plus arrival waits at the last slot's position.
 */
export class SlotAllocator {
  /** fileId → array of 3 walker ids (or null if free). */
  private slots = new Map<string, (string | null)[]>();

  /**
   * Acquire a slot for `walkerId` at building `fileId`. Returns the assigned
   * slot index (0-2) or -1 if all slots are occupied (4th+ arrival — must wait
   * at the last slot's position).
   *
   * Idempotent: if walkerId already occupies a slot at this building, returns
   * the existing slot index.
   */
  acquire(fileId: string, walkerId: string): number {
    let arr = this.slots.get(fileId);
    if (!arr) {
      arr = [null, null, null];
      this.slots.set(fileId, arr);
    }

    // Check if already occupying a slot (idempotent).
    const existing = arr.indexOf(walkerId);
    if (existing >= 0) return existing;

    // Find the lowest free slot.
    for (let i = 0; i < 3; i++) {
      if (arr[i] === null) {
        arr[i] = walkerId;
        return i;
      }
    }
    // All full → return -1 (4th+ arrival).
    return -1;
  }

  /**
   * Release `walkerId` from its slot at building `fileId`. No-op if walkerId
   * is not occupying a slot at this building.
   */
  release(fileId: string, walkerId: string): void {
    const arr = this.slots.get(fileId);
    if (!arr) return;
    for (let i = 0; i < 3; i++) {
      if (arr[i] === walkerId) {
        arr[i] = null;
        return;
      }
    }
  }

  /**
   * Sweep ALL slots for `walkerId` across every building. Call when a walker
   * despawns, to prevent leaked ids.
   */
  sweep(walkerId: string): void {
    for (const [, arr] of this.slots) {
      for (let i = 0; i < 3; i++) {
        if (arr[i] === walkerId) arr[i] = null;
      }
    }
  }

  /**
   * Get the world-space idle position for slot `index` (0-2) at building
   * `fileId`, given the building's door anchor (`door`: iso point) and the
   * incoming road direction (`dir`: vector pointing TOWARD the building along
   * the approach road). Slots extend BACKWARD along the approach (away from the
   * building threshold), so the queue lines up along the street, not inside.
   *
   * Slot i idles at `door - i × 12px` along `dir` (negated so slots extend
   * backward from the door). Overflow (-1) returns slot 2's position.
   */
  positionFor(
    index: number,
    door: IPoint,
    dir: IPoint,
  ): IPoint {
    const slot = Math.max(0, Math.min(2, index < 0 ? 2 : index));
    const dist = slot * 12;
    const len = Math.hypot(dir.x, dir.y) || 1;
    return {
      x: door.x - (dir.x / len) * dist,
      y: door.y - (dir.y / len) * dist,
    };
  }

  /** Clear all slots (scene teardown). */
  clear(): void {
    this.slots.clear();
  }
}
