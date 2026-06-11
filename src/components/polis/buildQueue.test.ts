// buildQueue.ts — viewport-prioritized build ordering (Phase B2b). PURE tests:
//   - priority items (visible+ring chunks) come FIRST, in DEPTH order (stable).
//   - non-priority items follow by Chebyshev chunk distance to the center, ties
//     broken by depth rank.
//   - the result is a permutation of [0, n) (covers every item exactly once).
//   - ring expansion + key predicate helpers.

import { describe, it, expect } from "vitest";
import {
  orderBuildQueue,
  priorityFromKeys,
  expandChunkRing,
  type BuildQueueItem,
} from "./buildQueue";

describe("orderBuildQueue — visible-first, depth-stable", () => {
  it("places priority-chunk items before out-of-view ones, preserving depth order", () => {
    // Depth order is the array order. Items 0 and 3 are in the priority chunk
    // (0,0); items 1,2,4 are out of view. Expect [0,3, …rest…].
    const items: BuildQueueItem[] = [
      { cx: 0, cy: 0 }, // 0 priority
      { cx: 5, cy: 5 }, // 1
      { cx: 9, cy: 9 }, // 2
      { cx: 0, cy: 0 }, // 3 priority (same chunk as 0)
      { cx: 6, cy: 6 }, // 4
    ];
    const priorityKeys = new Set(["0,0"]);
    const order = orderBuildQueue(
      items,
      priorityFromKeys(priorityKeys),
      { cx: 0, cy: 0 },
    );
    // Priority items first, in DEPTH order (0 before 3).
    expect(order.slice(0, 2)).toEqual([0, 3]);
    // Permutation of all indices.
    expect([...order].sort((a, b) => a - b)).toEqual([0, 1, 2, 3, 4]);
  });

  it("orders the non-priority remainder by chunk distance to the center, then depth", () => {
    const items: BuildQueueItem[] = [
      { cx: 10, cy: 0 }, // 0 — dist 10 from center (0,0)
      { cx: 2, cy: 0 }, // 1 — dist 2
      { cx: 5, cy: 0 }, // 2 — dist 5
      { cx: 2, cy: 0 }, // 3 — dist 2 (tie with 1; depth breaks → 1 before 3)
    ];
    // No priority region → everything is "rest", ordered by distance then depth.
    const order = orderBuildQueue(items, () => false, { cx: 0, cy: 0 });
    expect(order).toEqual([1, 3, 2, 0]);
  });

  it("is a stable permutation even when ALL items are priority (pure depth order)", () => {
    const items: BuildQueueItem[] = [
      { cx: 0, cy: 0 },
      { cx: 1, cy: 1 },
      { cx: 2, cy: 2 },
    ];
    const order = orderBuildQueue(items, () => true, { cx: 0, cy: 0 });
    expect(order).toEqual([0, 1, 2]);
  });

  it("handles an empty item list", () => {
    expect(orderBuildQueue([], () => true, { cx: 0, cy: 0 })).toEqual([]);
  });
});

describe("expandChunkRing", () => {
  it("ring 0 returns the base keys unchanged (as a fresh set)", () => {
    const base = new Set(["3,4"]);
    const out = expandChunkRing(base, 0);
    expect(out).not.toBe(base); // a copy, not the same reference
    expect([...out]).toEqual(["3,4"]);
  });

  it("ring 1 expands a single chunk into its 3x3 neighborhood", () => {
    const out = expandChunkRing(new Set(["0,0"]), 1);
    expect(out.size).toBe(9);
    for (let dx = -1; dx <= 1; dx++) {
      for (let dy = -1; dy <= 1; dy++) {
        expect(out.has(`${dx},${dy}`)).toBe(true);
      }
    }
  });

  it("ring 2 expands into a 5x5 neighborhood", () => {
    const out = expandChunkRing(new Set(["0,0"]), 2);
    expect(out.size).toBe(25);
    expect(out.has("2,2")).toBe(true);
    expect(out.has("-2,-2")).toBe(true);
    expect(out.has("3,0")).toBe(false);
  });

  it("merges overlapping neighborhoods (dedup) for adjacent base chunks", () => {
    const out = expandChunkRing(new Set(["0,0", "1,0"]), 1);
    // 0,0 → cols -1..1; 1,0 → cols 0..2; union cols -1..2 (4) × rows -1..1 (3) = 12.
    expect(out.size).toBe(12);
  });

  it("ignores malformed keys without throwing", () => {
    const out = expandChunkRing(new Set(["garbage", "0,0"]), 1);
    expect(out.size).toBe(9); // only 0,0 expanded
  });
});

describe("priorityFromKeys", () => {
  it("returns true only for chunks whose key is in the set", () => {
    const pred = priorityFromKeys(new Set(["1,2", "3,4"]));
    expect(pred(1, 2)).toBe(true);
    expect(pred(3, 4)).toBe(true);
    expect(pred(0, 0)).toBe(false);
  });
});
