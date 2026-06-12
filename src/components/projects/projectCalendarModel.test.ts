import { describe, expect, it } from "vitest";

import type { ProjectMilestone, ProjectSummary } from "../../types/backend";
import {
  addMilestoneArgs,
  flattenMilestones,
  formatDateHeading,
  groupMilestonesByDate,
  totalMilestoneCount,
} from "./projectCalendarModel";

function project(
  id: string,
  title: string,
  milestones: ProjectMilestone[],
): ProjectSummary {
  return {
    id,
    title,
    status: "active",
    updatedAt: "2026-06-01T00:00:00Z",
    rootPath: null,
    revision: "rev",
    path: `projects/${id}.md`,
    taskCounts: { todo: 0, wip: 0, review: 0, blocked: 0, done: 0, total: 0 },
    gitStatus: {} as ProjectSummary["gitStatus"],
    milestones,
  };
}

function milestone(
  id: string,
  title: string,
  date: string,
  note?: string | null,
): ProjectMilestone {
  return { id, title, date, note: note ?? null };
}

describe("groupMilestonesByDate", () => {
  it("returns an empty array when there are no milestones", () => {
    expect(groupMilestonesByDate([])).toEqual([]);
    expect(groupMilestonesByDate([project("p1", "P1", [])])).toEqual([]);
  });

  it("groups by date and sorts dates ascending", () => {
    const groups = groupMilestonesByDate([
      project("p1", "Alpha", [
        milestone("M1", "GA", "2026-09-01"),
        milestone("M2", "Beta", "2026-07-15"),
      ]),
    ]);
    expect(groups.map((g) => g.date)).toEqual(["2026-07-15", "2026-09-01"]);
    expect(groups[0].entries[0].title).toBe("Beta");
    expect(groups[1].entries[0].title).toBe("GA");
  });

  it("aggregates milestones across multiple projects into one date bucket", () => {
    const groups = groupMilestonesByDate([
      project("p1", "Alpha", [milestone("M1", "Cut", "2026-07-15")]),
      project("p2", "Beta", [milestone("M9", "Freeze", "2026-07-15")]),
    ]);
    expect(groups).toHaveLength(1);
    expect(groups[0].date).toBe("2026-07-15");
    expect(groups[0].entries).toHaveLength(2);
    // Stable order: by project title (Alpha before Beta).
    expect(groups[0].entries[0].projectTitle).toBe("Alpha");
    expect(groups[0].entries[1].projectTitle).toBe("Beta");
    // Each entry is tagged with its project.
    expect(groups[0].entries[0].projectId).toBe("p1");
    expect(groups[0].entries[1].projectId).toBe("p2");
  });

  it("carries a stable composite key and the optional note", () => {
    const [group] = groupMilestonesByDate([
      project("p1", "Alpha", [milestone("M1", "Cut", "2026-07-15", "be ready")]),
    ]);
    expect(group.entries[0].key).toBe("p1:M1");
    expect(group.entries[0].note).toBe("be ready");
  });

  it("drops malformed dates, blank titles, and blank notes defensively", () => {
    const groups = groupMilestonesByDate([
      project("p1", "Alpha", [
        milestone("M1", "Bad date", "2026/07/15"),
        milestone("M2", "   ", "2026-07-15"),
        milestone("M3", "Good", "2026-07-15", "   "),
      ]),
    ]);
    expect(groups).toHaveLength(1);
    expect(groups[0].entries).toHaveLength(1);
    expect(groups[0].entries[0].title).toBe("Good");
    expect(groups[0].entries[0].note).toBeNull();
  });

  it("treats a missing milestones field as empty", () => {
    const p = project("p1", "Alpha", []);
    delete (p as { milestones?: unknown }).milestones;
    expect(groupMilestonesByDate([p])).toEqual([]);
  });
});

describe("flattenMilestones / totalMilestoneCount", () => {
  it("counts valid milestones across projects", () => {
    const projects = [
      project("p1", "Alpha", [
        milestone("M1", "A", "2026-07-15"),
        milestone("M2", "B", "2026-08-15"),
      ]),
      project("p2", "Beta", [milestone("M3", "C", "2026-09-15")]),
    ];
    expect(flattenMilestones(projects)).toHaveLength(3);
    expect(totalMilestoneCount(projects)).toBe(3);
  });

  it("excludes invalid entries from the count", () => {
    const projects = [
      project("p1", "Alpha", [
        milestone("M1", "ok", "2026-07-15"),
        milestone("M2", "bad", "nope"),
      ]),
    ];
    expect(totalMilestoneCount(projects)).toBe(1);
  });
});

describe("addMilestoneArgs", () => {
  const base = { projectId: "p1", title: "Cut", date: "2026-07-15" };

  it("builds camelCase args and omits a blank note", () => {
    const result = addMilestoneArgs({ ...base, note: "  " });
    expect(result).toEqual({
      ok: true,
      args: { projectId: "p1", title: "Cut", date: "2026-07-15", note: null },
    });
  });

  it("trims fields and keeps a non-blank note", () => {
    const result = addMilestoneArgs({
      projectId: " p1 ",
      title: " Cut ",
      date: " 2026-07-15 ",
      note: " be ready ",
    });
    expect(result).toEqual({
      ok: true,
      args: { projectId: "p1", title: "Cut", date: "2026-07-15", note: "be ready" },
    });
  });

  it("requires project, title and date", () => {
    expect(addMilestoneArgs({ ...base, projectId: "" }).ok).toBe(false);
    expect(addMilestoneArgs({ ...base, title: " " }).ok).toBe(false);
    expect(addMilestoneArgs({ ...base, date: "" }).ok).toBe(false);
  });

  it("W8: rejects a malformed date BEFORE the IPC round-trip with a clear error", () => {
    for (const bad of ["2026/07/15", "15-07-2026", "2026-7-5", "not-a-date"]) {
      const result = addMilestoneArgs({ ...base, date: bad });
      expect(result.ok).toBe(false);
      if (!result.ok) {
        expect(result.error).toBe("Milestone date must use YYYY-MM-DD.");
      }
    }
  });
});

describe("formatDateHeading", () => {
  it("formats a valid ISO date without timezone drift", () => {
    expect(formatDateHeading("2026-07-15")).toBe("Wed, Jul 15, 2026");
  });

  it("returns the raw string for a malformed date", () => {
    expect(formatDateHeading("not-a-date")).toBe("not-a-date");
  });
});
