import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";

// The container imports invokeBackendCommand from AppContext (which pulls in the
// whole app context). Mock it so the test stays a pure unit (node env, no Tauri).
vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: vi.fn(async () => undefined),
  isTauriRuntime: () => false,
}));

import type { ProjectMilestone, ProjectSummary } from "../../types/backend";
import { ProjectCalendar } from "./ProjectCalendar";
import { addMilestoneArgs, removeMilestoneArgs } from "./projectCalendarModel";

function project(
  id: string,
  title: string,
  milestones: ProjectMilestone[],
  status: ProjectSummary["status"] = "active",
): ProjectSummary {
  return {
    id,
    title,
    status,
    updatedAt: "2026-06-01T00:00:00Z",
    rootPath: null,
    revision: "rev",
    path: `projects/${id}.md`,
    taskCounts: { todo: 0, wip: 0, review: 0, blocked: 0, done: 0, total: 0 },
    gitStatus: {} as ProjectSummary["gitStatus"],
    milestones,
  };
}

const noop = () => undefined;

describe("ProjectCalendar render", () => {
  it("renders the empty state when no project has milestones", () => {
    const html = renderToStaticMarkup(
      <ProjectCalendar
        projects={[project("p1", "Alpha", [])]}
        onSelectProject={noop}
        onChanged={noop}
      />,
    );
    expect(html).toContain("No milestones yet");
  });

  it("renders milestone entries with their project name", () => {
    const html = renderToStaticMarkup(
      <ProjectCalendar
        projects={[
          project("p1", "Alpha", [
            { id: "M1", title: "GA release", date: "2026-09-01", note: "ship" },
          ]),
        ]}
        onSelectProject={noop}
        onChanged={noop}
      />,
    );
    expect(html).toContain("GA release");
    expect(html).toContain("Alpha");
    expect(html).toContain("ship");
    expect(html).not.toContain("No milestones yet");
  });

  it("disables the add affordance when there is no targetable project", () => {
    const html = renderToStaticMarkup(
      <ProjectCalendar
        projects={[project("p1", "Archived", [], "archived")]}
        onSelectProject={noop}
        onChanged={noop}
      />,
    );
    // No active project to attach a milestone to → the add button is disabled.
    expect(html).toContain("disabled");
  });
});

describe("ProjectCalendar IPC arg builders", () => {
  it("builds camelCase add args and omits a blank note as null", () => {
    const built = addMilestoneArgs({
      projectId: " p1 ",
      title: " GA ",
      date: " 2026-09-01 ",
      note: "  ",
    });
    expect(built.ok).toBe(true);
    if (built.ok) {
      expect(built.args).toEqual({
        projectId: "p1",
        title: "GA",
        date: "2026-09-01",
        note: null,
      });
    }
  });

  it("carries a non-blank note through", () => {
    const built = addMilestoneArgs({
      projectId: "p1",
      title: "GA",
      date: "2026-09-01",
      note: "ship it",
    });
    expect(built.ok && built.args.note).toBe("ship it");
  });

  it("rejects missing project, title, or date with a message", () => {
    expect(addMilestoneArgs({ projectId: "", title: "x", date: "2026-09-01" })).toEqual({
      ok: false,
      error: "Pick a project for the milestone.",
    });
    expect(addMilestoneArgs({ projectId: "p1", title: " ", date: "2026-09-01" })).toEqual({
      ok: false,
      error: "Milestone title is required.",
    });
    expect(addMilestoneArgs({ projectId: "p1", title: "x", date: "" })).toEqual({
      ok: false,
      error: "Milestone date is required.",
    });
  });

  it("builds camelCase remove args", () => {
    expect(removeMilestoneArgs("p1", "M1")).toEqual({
      projectId: "p1",
      milestoneId: "M1",
    });
  });
});
