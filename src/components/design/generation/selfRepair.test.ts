import { describe, it, expect } from "vitest";
import {
  shouldSelfRepair,
  buildRepairPrompt,
  MAX_REPAIR_ATTEMPTS,
  DEFAULT_REPAIR_RETRIES,
  type RepairableOutcome,
} from "./selfRepair";

function outcome(
  committedNodeCount: number,
  codes: Array<"FOSTER_PARENTED_ROOT" | "EMPTY"> = [],
): RepairableOutcome {
  return {
    committedNodeCount,
    remainingViolations: codes.map((code) => ({ code, message: "" })),
  };
}

describe("shouldSelfRepair", () => {
  it("retries when zero nodes were committed (within cap)", () => {
    expect(shouldSelfRepair(outcome(0), 0)).toBe(true);
  });

  it("retries when a node was dropped for an unfixable violation", () => {
    expect(shouldSelfRepair(outcome(2, ["FOSTER_PARENTED_ROOT"]), 0)).toBe(true);
  });

  it("does NOT retry a clean outcome", () => {
    expect(shouldSelfRepair(outcome(3), 0)).toBe(false);
  });

  it("does NOT retry once the configured retry count is reached", () => {
    // Default 1 retry: after 1 attempt, no more.
    expect(shouldSelfRepair(outcome(0), 1, 1)).toBe(false);
  });

  it("never exceeds the HARD cap even if maxRetries is huge", () => {
    expect(shouldSelfRepair(outcome(0), MAX_REPAIR_ATTEMPTS, 999)).toBe(false);
    // One below the hard cap still allows a retry.
    expect(shouldSelfRepair(outcome(0), MAX_REPAIR_ATTEMPTS - 1, 999)).toBe(true);
  });

  it("with maxRetries=0 never retries", () => {
    expect(shouldSelfRepair(outcome(0), 0, 0)).toBe(false);
  });

  it("clamps a negative maxRetries to 0 (no retry)", () => {
    expect(shouldSelfRepair(outcome(0), 0, -5)).toBe(false);
  });

  it("DEFAULT_REPAIR_RETRIES is within the hard cap", () => {
    expect(DEFAULT_REPAIR_RETRIES).toBeLessThanOrEqual(MAX_REPAIR_ATTEMPTS);
  });
});

describe("buildRepairPrompt", () => {
  it("includes the original instruction + correction for a dropped node", () => {
    const p = buildRepairPrompt(
      "a pricing section",
      outcome(2, ["FOSTER_PARENTED_ROOT"]),
      "",
    );
    expect(p).not.toBeNull();
    expect(p).toContain("a pricing section");
    expect(p).toContain("<tr>"); // foster-parent correction line
  });

  it("merges existing context before the correction", () => {
    const p = buildRepairPrompt(
      "x",
      outcome(0),
      "CONTEXT-BLOCK-TOKEN",
    );
    expect(p).toContain("CONTEXT-BLOCK-TOKEN");
  });

  it("falls back to an EMPTY correction when nothing was produced", () => {
    const p = buildRepairPrompt("x", outcome(0), "");
    expect(p).not.toBeNull();
    expect(p).toContain("non-empty UI markup");
  });

  it("returns null when there is nothing actionable", () => {
    // committed > 0 and no violations -> the caller shouldn't have asked.
    expect(buildRepairPrompt("x", outcome(3), "")).toBeNull();
  });

  it("is deterministic for the same inputs", () => {
    const a = buildRepairPrompt("x", outcome(0, ["FOSTER_PARENTED_ROOT"]), "ctx");
    const b = buildRepairPrompt("x", outcome(0, ["FOSTER_PARENTED_ROOT"]), "ctx");
    expect(a).toBe(b);
  });
});
