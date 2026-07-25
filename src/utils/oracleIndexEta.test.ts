import { describe, it, expect } from "vitest";
import {
  estimateOracleIndexEta,
  formatOracleIndexEta,
  normalizeIndexPhase,
  type OracleIndexProgressSample,
} from "./oracleIndexEta";

// Helper: build evenly spaced samples that advance by `step` files each tick.
function buildSamples(opts: {
  startCount: number;
  step: number;
  n: number;
  startAt: number;
  intervalMs: number;
  phase: string;
}): OracleIndexProgressSample[] {
  const out: OracleIndexProgressSample[] = [];
  for (let i = 0; i < opts.n; i++) {
    out.push({
      count: opts.startCount + i * opts.step,
      at: opts.startAt + i * opts.intervalMs,
      phase: opts.phase,
    });
  }
  return out;
}

describe("normalizeIndexPhase", () => {
  it("defaults missing / blank values to running", () => {
    expect(normalizeIndexPhase(undefined)).toBe("running");
    expect(normalizeIndexPhase(null)).toBe("running");
    expect(normalizeIndexPhase("")).toBe("running");
    expect(normalizeIndexPhase("   ")).toBe("running");
    expect(normalizeIndexPhase(12)).toBe("running");
  });

  it("trims known phase strings", () => {
    expect(normalizeIndexPhase("embedding")).toBe("embedding");
    expect(normalizeIndexPhase(" cooling_gpu ")).toBe("cooling_gpu");
  });
});

describe("formatOracleIndexEta", () => {
  it("rounds to coarse minute / hour buckets and never renders NaN", () => {
    expect(formatOracleIndexEta(30_000)).toBe("~1 min left");
    expect(formatOracleIndexEta(89_000)).toBe("~1 min left");
    expect(formatOracleIndexEta(4 * 60_000)).toBe("~4 min left");
    expect(formatOracleIndexEta(90 * 60_000)).toBe("~1.5 h left");
    expect(formatOracleIndexEta(2 * 60 * 60_000)).toBe("~2 h left");
    expect(formatOracleIndexEta(Number.NaN)).toBe("estimating…");
    expect(formatOracleIndexEta(Number.POSITIVE_INFINITY)).toBe("estimating…");
    expect(formatOracleIndexEta(-5_000)).toBe("estimating…");
  });
});

describe("estimateOracleIndexEta", () => {
  const base = {
    expectedFiles: 1000,
    currentCount: 100,
    phase: "running" as unknown,
    now: 100_000,
    prevRemainingMs: null as number | null,
    stalled: false,
  };

  it("derives rate from a recent window, not the whole run", () => {
    // Early phase: very slow (1 file / 10s) for a long stretch.
    const slow = buildSamples({
      startCount: 0,
      step: 1,
      n: 10,
      startAt: 0,
      intervalMs: 10_000,
      phase: "running",
    });
    // Recent window: fast (10 files / 3s).
    const fast = buildSamples({
      startCount: 200,
      step: 10,
      n: 6,
      startAt: 200_000,
      intervalMs: 3_000,
      phase: "running",
    });
    const samples = [...slow, ...fast];
    const now = 200_000 + 5 * 3_000;
    const currentCount = 200 + 5 * 10; // 250

    const result = estimateOracleIndexEta({
      ...base,
      samples,
      currentCount,
      now,
      expectedFiles: 1000,
    });

    expect(result.kind).toBe("eta");
    if (result.kind !== "eta") return;

    // Recent rate ≈ 10/3000 files/ms → remaining 750 files → ~225_000 ms ≈ ~4 min.
    // Whole-run average would be far slower (~tens of minutes). Assert we are
    // in the recent-window ballpark, not the whole-run ballpark.
    expect(result.remainingMs).toBeLessThan(8 * 60_000);
    expect(result.remainingMs).toBeGreaterThan(60_000);
    expect(result.label).toMatch(/^~\d+ min left$/);
  });

  it("does not poison the estimate with the previous phase's rate", () => {
    // Fast file-scan samples.
    const scan = buildSamples({
      startCount: 0,
      step: 20,
      n: 8,
      startAt: 0,
      intervalMs: 2_000,
      phase: "running",
    });
    // Slow embedding samples (1 file / 15s).
    const embed = buildSamples({
      startCount: 100,
      step: 1,
      n: 6,
      startAt: 50_000,
      intervalMs: 15_000,
      phase: "embedding",
    });
    const samples = [...scan, ...embed];
    const now = 50_000 + 5 * 15_000;
    const currentCount = 100 + 5;

    const result = estimateOracleIndexEta({
      ...base,
      samples,
      currentCount,
      phase: "embedding",
      now,
      expectedFiles: 200,
      // Even if the UI still held a scan-era remaining, the pure function must
      // not use it when the caller resets after phase change (null).
      prevRemainingMs: null,
    });

    expect(result.kind).toBe("eta");
    if (result.kind !== "eta") return;

    // Embedding rate ≈ 1/15_000 → remaining 95 files → ~1_425_000 ms ≈ ~24 min.
    // Scan-rate remaining would be ~seconds. Must be the slow embedding rate.
    expect(result.remainingMs).toBeGreaterThan(15 * 60_000);
  });

  it("produces no estimate from too few samples", () => {
    const samples = buildSamples({
      startCount: 10,
      step: 5,
      n: 2, // only two points
      startAt: 0,
      intervalMs: 3_000,
      phase: "running",
    });
    const result = estimateOracleIndexEta({
      ...base,
      samples,
      currentCount: 15,
      now: 3_000,
    });
    expect(result).toEqual({ kind: "estimating", label: "estimating…" });
  });

  it("withholds until enough positive deltas (warm-up guard)", () => {
    // Four samples but only one advance → still estimating.
    const samples: OracleIndexProgressSample[] = [
      { count: 50, at: 0, phase: "embedding" },
      { count: 50, at: 3_000, phase: "embedding" },
      { count: 50, at: 6_000, phase: "embedding" },
      { count: 51, at: 9_000, phase: "embedding" }, // single advance
    ];
    const result = estimateOracleIndexEta({
      ...base,
      samples,
      currentCount: 51,
      phase: "embedding",
      now: 9_000,
      expectedFiles: 1232,
    });
    expect(result.kind).toBe("estimating");
  });

  it("reports paused/waiting on cooling_gpu instead of a diverging number", () => {
    const samples = buildSamples({
      startCount: 100,
      step: 5,
      n: 6,
      startAt: 0,
      intervalMs: 3_000,
      phase: "running",
    });
    // Zero-progress stretch while cooling — samples freeze at last count.
    samples.push(
      { count: 125, at: 20_000, phase: "cooling_gpu" },
      { count: 125, at: 25_000, phase: "cooling_gpu" },
      { count: 125, at: 30_000, phase: "cooling_gpu" },
      { count: 125, at: 35_000, phase: "cooling_gpu" },
    );

    const result = estimateOracleIndexEta({
      ...base,
      samples,
      currentCount: 125,
      phase: "cooling_gpu",
      now: 35_000,
      // A naive rate of 0 would yield Infinity; we must not.
      prevRemainingMs: 600_000,
    });

    expect(result).toEqual({ kind: "paused", label: "paused — waiting…" });
  });

  it("reports paused when the existing stall detector fires", () => {
    const samples = buildSamples({
      startCount: 50,
      step: 1,
      n: 6,
      startAt: 0,
      intervalMs: 3_000,
      phase: "running",
    });
    const result = estimateOracleIndexEta({
      ...base,
      samples,
      currentCount: 55,
      now: 100_000,
      stalled: true,
    });
    expect(result).toEqual({ kind: "paused", label: "paused — waiting…" });
  });

  it("degrades safely for expectedFiles=0, backwards counts, and a single sample", () => {
    const one: OracleIndexProgressSample[] = [
      { count: 10, at: 0, phase: "running" },
    ];

    expect(
      estimateOracleIndexEta({
        ...base,
        samples: one,
        expectedFiles: 0,
        currentCount: 0,
        now: 1_000,
      }).kind,
    ).toBe("none");

    // Counts going backwards (re-index / recount glitch) → no crash, no NaN.
    const backwards = buildSamples({
      startCount: 200,
      step: -10,
      n: 6,
      startAt: 0,
      intervalMs: 3_000,
      phase: "running",
    });
    const backResult = estimateOracleIndexEta({
      ...base,
      samples: backwards,
      currentCount: 150,
      now: 15_000,
      expectedFiles: 1000,
    });
    expect(backResult.kind === "eta" ? backResult.label : backResult.kind).not.toMatch(
      /NaN|Infinity/i,
    );
    // Zero/negative rate window → estimating, not Infinity.
    expect(backResult.kind).toBe("estimating");

    const single = estimateOracleIndexEta({
      ...base,
      samples: one,
      currentCount: 10,
      now: 1_000,
      expectedFiles: 1000,
    });
    expect(single).toEqual({ kind: "estimating", label: "estimating…" });
  });

  it("smooths successive estimates so the value does not thrash", () => {
    const samples = buildSamples({
      startCount: 100,
      step: 5,
      n: 8,
      startAt: 0,
      intervalMs: 3_000,
      phase: "running",
    });
    const now = 7 * 3_000;
    const currentCount = 100 + 7 * 5;

    const first = estimateOracleIndexEta({
      ...base,
      samples,
      currentCount,
      now,
      prevRemainingMs: null,
      expectedFiles: 1000,
    });
    expect(first.kind).toBe("eta");
    if (first.kind !== "eta") return;

    // Inject a wildly slower recent hop that would 10× the raw ETA if unsmoothed.
    const noisy = [
      ...samples,
      { count: currentCount + 1, at: now + 60_000, phase: "running" },
    ];
    const second = estimateOracleIndexEta({
      ...base,
      samples: noisy,
      currentCount: currentCount + 1,
      now: now + 60_000,
      prevRemainingMs: first.remainingMs,
      expectedFiles: 1000,
    });
    expect(second.kind).toBe("eta");
    if (second.kind !== "eta") return;

    // Must stay within the hard clamp band of the previous estimate.
    expect(second.remainingMs).toBeLessThanOrEqual(first.remainingMs * 1.5 + 1);
    expect(second.remainingMs).toBeGreaterThanOrEqual(first.remainingMs * 0.55 - 1);
  });

  it("returns none when already complete (no remaining files)", () => {
    const samples = buildSamples({
      startCount: 990,
      step: 2,
      n: 6,
      startAt: 0,
      intervalMs: 3_000,
      phase: "running",
    });
    expect(
      estimateOracleIndexEta({
        ...base,
        samples,
        currentCount: 1000,
        expectedFiles: 1000,
        now: 15_000,
      }).kind,
    ).toBe("none");
  });
});
