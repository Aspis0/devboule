import { describe, it, expect } from "vitest";
import { sliceBatches, DEFAULT_BUILD_BATCH } from "./chunk";

describe("sliceBatches", () => {
  it("returns no ranges for an empty set", () => {
    expect(sliceBatches(0, 4)).toEqual([]);
    expect(sliceBatches(-5, 4)).toEqual([]);
  });

  it("returns a single full range when total <= batchSize", () => {
    expect(sliceBatches(4, 4)).toEqual([{ start: 0, end: 4 }]);
    expect(sliceBatches(3, 10)).toEqual([{ start: 0, end: 3 }]);
  });

  it("splits into contiguous half-open ranges with a short final batch", () => {
    expect(sliceBatches(10, 4)).toEqual([
      { start: 0, end: 4 },
      { start: 4, end: 8 },
      { start: 8, end: 10 },
    ]);
  });

  it("covers every index exactly once, in order, with no gaps or overlaps", () => {
    const total = 1003;
    const ranges = sliceBatches(total, DEFAULT_BUILD_BATCH);
    // Contiguous + ordered.
    expect(ranges[0].start).toBe(0);
    expect(ranges[ranges.length - 1].end).toBe(total);
    for (let i = 1; i < ranges.length; i++) {
      expect(ranges[i].start).toBe(ranges[i - 1].end);
    }
    // Every batch (except possibly the last) is exactly the batch size, and none
    // exceeds it.
    for (const r of ranges) {
      expect(r.end - r.start).toBeGreaterThan(0);
      expect(r.end - r.start).toBeLessThanOrEqual(DEFAULT_BUILD_BATCH);
    }
    // Total coverage.
    const covered = ranges.reduce((n, r) => n + (r.end - r.start), 0);
    expect(covered).toBe(total);
  });

  it("floors a degenerate batch size at 1 (never spins forever)", () => {
    expect(sliceBatches(3, 0)).toEqual([
      { start: 0, end: 1 },
      { start: 1, end: 2 },
      { start: 2, end: 3 },
    ]);
    expect(sliceBatches(2, -10)).toEqual([
      { start: 0, end: 1 },
      { start: 1, end: 2 },
    ]);
  });
});
