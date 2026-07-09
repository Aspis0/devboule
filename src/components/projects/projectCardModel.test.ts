import { describe, expect, it } from "vitest";
import {
  folderBasename,
  taskCountsLine,
  nextMilestone,
  TASK_BREAKDOWN_ORDER,
} from "./projectCardModel";
import type { ProjectMilestone, ProjectTaskCounts } from "../../types/backend";

describe("folderBasename", () => {
  it("handles a POSIX path", () => {
    expect(folderBasename("/Users/user/Projects/Alpha")).toBe("Alpha");
  });

  it("handles a Windows path with backslashes", () => {
    expect(folderBasename("C:\\Users\\user\\Projects\\Beta")).toBe("Beta");
  });

  it("trims a trailing separator", () => {
    expect(folderBasename("/Users/user/Projects/Alpha/")).toBe("Alpha");
    expect(folderBasename("C:\\Projects\\Beta\\")).toBe("Beta");
  });

  it("returns null for a null rootPath", () => {
    expect(folderBasename(null)).toBeNull();
  });

  it("returns null for an empty-string rootPath", () => {
    expect(folderBasename("")).toBeNull();
  });
});

describe("taskCountsLine", () => {
  it("returns null when total is 0 (component renders 'no tasks yet')", () => {
    const counts: ProjectTaskCounts = {
      todo: 0,
      wip: 0,
      review: 0,
      blocked: 0,
      done: 0,
      total: 0,
    };
    expect(taskCountsLine(counts)).toBeNull();
  });

  it("returns only non-zero states in the fixed order", () => {
    const counts: ProjectTaskCounts = {
      todo: 9, // intentionally omitted from the compact line
      wip: 2,
      review: 1,
      blocked: 1,
      done: 5,
      total: 18,
    };
    expect(taskCountsLine(counts)).toBe("2 wip · 1 review · 1 blocked · 5 done");
  });

  it("handles a blocked-only case", () => {
    const counts: ProjectTaskCounts = {
      todo: 0,
      wip: 0,
      review: 0,
      blocked: 3,
      done: 0,
      total: 3,
    };
    expect(taskCountsLine(counts)).toBe("3 blocked");
  });

  it("omits todo (the default/background state)", () => {
    const counts: ProjectTaskCounts = {
      todo: 7,
      wip: 0,
      review: 0,
      blocked: 0,
      done: 0,
      total: 7,
    };
    expect(taskCountsLine(counts)).toBeNull();
  });
});

describe("nextMilestone", () => {
  const today = new Date(2026, 6, 9); // 2026-07-09 (local)

  const ms = (
    id: string,
    date: string,
    title: string,
  ): ProjectMilestone => ({ id, date, title });

  it("returns null when there are no milestones", () => {
    expect(nextMilestone(undefined, today)).toBeNull();
    expect(nextMilestone([], today)).toBeNull();
  });

  it("picks a single future milestone", () => {
    expect(nextMilestone([ms("m1", "2026-07-15", "Ship v1")], today)).toEqual({
      title: "Ship v1",
      date: "2026-07-15",
      overdue: false,
    });
  });

  it("picks the soonest of multiple future milestones", () => {
    const result = nextMilestone(
      [
        ms("m1", "2026-07-20", "Later"),
        ms("m2", "2026-07-15", "Sooner"),
      ],
      today,
    );
    expect(result).toEqual({
      title: "Sooner",
      date: "2026-07-15",
      overdue: false,
    });
  });

  it("flags the most recent overdue milestone when nothing is upcoming", () => {
    const result = nextMilestone([ms("m1", "2026-07-03", "Old")], today);
    expect(result).toEqual({
      title: "Old",
      date: "2026-07-03",
      overdue: true,
    });
  });

  it("prefers an upcoming milestone over an overdue one (not overdue)", () => {
    const result = nextMilestone(
      [ms("m1", "2026-07-03", "Old"), ms("m2", "2026-07-15", "Next")],
      today,
    );
    expect(result).toEqual({
      title: "Next",
      date: "2026-07-15",
      overdue: false,
    });
  });
});

describe("TASK_BREAKDOWN_ORDER", () => {
  it("lists wip, review, blocked, done in that fixed order", () => {
    expect(TASK_BREAKDOWN_ORDER.map((s) => s.key)).toEqual([
      "wip",
      "review",
      "blocked",
      "done",
    ]);
  });
});
