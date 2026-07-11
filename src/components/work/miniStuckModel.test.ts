import { describe, it, expect } from "vitest";
import { stuckReasonLabel, filterStuckReports } from "./miniStuckModel";
import type { MiniStuckReport } from "./miniStuckModel";

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

function report(overrides: Partial<MiniStuckReport> = {}): MiniStuckReport {
  return {
    taskId: "T1",
    agentId: "agent-1",
    reason: "timeout",
    attempts: 1,
    lastOutput: "",
    filesTouched: [],
    ...overrides,
  };
}

describe("filterStuckReports", () => {
  it("shows reports whose projectId matches the current project", () => {
    const reports = [
      report({ taskId: "T1", projectId: "p1" }),
      report({ taskId: "T2", projectId: "p1" }),
    ];
    expect(filterStuckReports(reports, "p1")).toHaveLength(2);
  });

  it("hides reports whose projectId does not match", () => {
    const reports = [
      report({ taskId: "T1", projectId: "p1" }),
      report({ taskId: "T2", projectId: "p2" }),
    ];
    expect(filterStuckReports(reports, "p1")).toHaveLength(1);
    expect(filterStuckReports(reports, "p1")[0].taskId).toBe("T1");
  });

  it("shows reports with null projectId (legacy safety)", () => {
    const reports = [
      report({ taskId: "T1", projectId: null }),
      report({ taskId: "T2", projectId: "p2" }),
    ];
    expect(filterStuckReports(reports, "p1")).toHaveLength(1);
    expect(filterStuckReports(reports, "p1")[0].taskId).toBe("T1");
  });

  it("shows reports with undefined projectId (legacy safety)", () => {
    const reports = [
      report({ taskId: "T1" }), // projectId omitted → undefined
      report({ taskId: "T2", projectId: "p2" }),
    ];
    expect(filterStuckReports(reports, "p1")).toHaveLength(1);
    expect(filterStuckReports(reports, "p1")[0].taskId).toBe("T1");
  });

  it("returns empty array when no reports match", () => {
    const reports = [
      report({ taskId: "T1", projectId: "p2" }),
      report({ taskId: "T2", projectId: "p3" }),
    ];
    expect(filterStuckReports(reports, "p1")).toHaveLength(0);
  });

  it("does not mutate the input array", () => {
    const reports = [
      report({ taskId: "T1", projectId: "p1" }),
      report({ taskId: "T2", projectId: "p2" }),
    ];
    filterStuckReports(reports, "p1");
    expect(reports).toHaveLength(2);
  });
});
