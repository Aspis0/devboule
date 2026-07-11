import { describe, it, expect } from "vitest";
import { stuckReasonLabel } from "./miniStuckModel";

describe("stuckReasonLabel", () => {
  it("returns 'timed out' for timeout", () => {
    expect(stuckReasonLabel("timeout")).toBe("timed out");
  });

  it("returns 'failed' for failed", () => {
    expect(stuckReasonLabel("failed")).toBe("failed");
  });

  it("passes through an unknown string", () => {
    expect(stuckReasonLabel("loop")).toBe("loop");
  });

  it("returns 'stuck' for empty string", () => {
    expect(stuckReasonLabel("")).toBe("stuck");
  });
});
