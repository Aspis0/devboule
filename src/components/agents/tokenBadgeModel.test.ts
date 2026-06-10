import { describe, it, expect } from "vitest";
import {
  formatTokenCount,
  formatCostUsd,
  tokenBadgeView,
} from "./tokenBadgeModel";
import type { AgentTokenUsage } from "../../types/backend";

describe("formatTokenCount", () => {
  it("formats millions with one decimal, trimming .0", () => {
    expect(formatTokenCount(1_200_000)).toBe("1.2M");
    expect(formatTokenCount(2_000_000)).toBe("2M");
  });

  it("formats thousands as k", () => {
    expect(formatTokenCount(850_000)).toBe("850k");
    expect(formatTokenCount(1_500)).toBe("1.5k");
  });

  it("shows raw int below 1000 and 0 for non-positive/non-finite", () => {
    expect(formatTokenCount(42)).toBe("42");
    expect(formatTokenCount(999)).toBe("999");
    expect(formatTokenCount(0)).toBe("0");
    expect(formatTokenCount(-5)).toBe("0");
    expect(formatTokenCount(Number.NaN)).toBe("0");
  });

  it("promotes k to M when the rounded k value reaches 1000 (no '1000k')", () => {
    // FIX 6: the confirmed bug — an input whose one-decimal k rounds to 1000 used to
    // render the nonsensical "1000k". It must roll over to the M range instead. With
    // the existing M trim (".0" dropped), 999_950 -> 1.0M -> "1M".
    expect(formatTokenCount(999_950)).toBe("1M");
    expect(formatTokenCount(1_000_000)).toBe("1M");
    // Just below the rollover the one-decimal k still renders (no premature promote).
    expect(formatTokenCount(999_499)).toBe("999.5k");
    expect(formatTokenCount(999_500)).toBe("999.5k");
    // The chip never emits a "1000k" string for any input.
    for (const v of [999_000, 999_499, 999_500, 999_950, 1_000_000]) {
      expect(formatTokenCount(v)).not.toContain("1000k");
    }
  });
});

describe("formatCostUsd", () => {
  it("formats a dollar amount with two decimals", () => {
    expect(formatCostUsd(3.4)).toBe("$3.40");
    expect(formatCostUsd(12.345)).toBe("$12.35");
  });

  it("rounds a tiny non-zero cost up to one visible cent", () => {
    expect(formatCostUsd(0.005)).toBe("$0.01");
  });

  it("shows $0.00 for zero and null for null/non-finite", () => {
    expect(formatCostUsd(0)).toBe("$0.00");
    expect(formatCostUsd(null)).toBeNull();
    expect(formatCostUsd(Number.POSITIVE_INFINITY)).toBeNull();
  });
});

const claudeUsage = (total: number, costUsd: number | null): AgentTokenUsage => ({
  tokens: { input: 0, output: 0, cacheCreation: 0, cacheRead: 0, total },
  costUsd,
  source: "claude-transcript",
});

describe("tokenBadgeView", () => {
  it("renders tokens + cost for a claude transcript", () => {
    const view = tokenBadgeView(claudeUsage(1_200_000, 3.4));
    expect(view.hidden).toBe(false);
    expect(view.tone).toBe("claude");
    expect(view.text).toBe("1.2M tok · $3.40");
  });

  it("renders tokens only when the cost is unknown (null)", () => {
    const view = tokenBadgeView(claudeUsage(850_000, null));
    expect(view.hidden).toBe(false);
    expect(view.text).toBe("850k tok");
  });

  it("renders a flat 'subscription' label with no cost", () => {
    const view = tokenBadgeView({
      tokens: { input: 0, output: 0, cacheCreation: 0, cacheRead: 0, total: 0 },
      costUsd: null,
      source: "subscription",
    });
    expect(view.hidden).toBe(false);
    expect(view.tone).toBe("subscription");
    expect(view.text).toBe("subscription");
  });

  it("hides for unavailable and for null/undefined usage", () => {
    expect(
      tokenBadgeView({
        tokens: { input: 0, output: 0, cacheCreation: 0, cacheRead: 0, total: 0 },
        costUsd: null,
        source: "unavailable",
      }).hidden,
    ).toBe(true);
    expect(tokenBadgeView(null).hidden).toBe(true);
    expect(tokenBadgeView(undefined).hidden).toBe(true);
  });
});
