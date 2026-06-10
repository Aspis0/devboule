// Pure batching helper for the Polis non-blocking city build.
//
// Building a large city's geometry (thousands of procedural PIXI kit buildings)
// in a single synchronous loop blocks the UI thread for minutes. The renderer
// instead processes buildings in fixed-size BATCHES, yielding to the event loop
// between each so the browser can paint the loading overlay and stay responsive.
//
// This module is the pure, deterministic, PIXI-free core of that batching: given
// a total count and a batch size it returns the [start, end) index ranges to
// process, in order, covering every index exactly once. Trivially unit-testable.

/** A half-open index range [start, end) into the building array. */
export interface BatchRange {
  start: number;
  end: number;
}

/** Default buildings processed per animation frame. Tuned so each batch stays
 *  well under a frame budget on a large city while keeping total build time
 *  reasonable: a few hundred kit buildings per ~16ms frame. */
export const DEFAULT_BUILD_BATCH = 150;

/**
 * Partition `total` items into ordered, contiguous, non-overlapping half-open
 * ranges of at most `batchSize` items each. Covers [0, total) exactly once.
 *
 *   sliceBatches(10, 4) -> [{0,4},{4,8},{8,10}]
 *   sliceBatches(0, 4)  -> []
 *   sliceBatches(4, 4)  -> [{0,4}]
 *
 * `batchSize` is floored at 1 so a degenerate size can't spin forever.
 */
export function sliceBatches(total: number, batchSize: number): BatchRange[] {
  const ranges: BatchRange[] = [];
  if (total <= 0) return ranges;
  const step = Math.max(1, Math.floor(batchSize));
  for (let start = 0; start < total; start += step) {
    ranges.push({ start, end: Math.min(start + step, total) });
  }
  return ranges;
}
