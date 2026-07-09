import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { ProjectSummary, ProjectGitStatus } from "../../types/backend";
import { ProjectCard } from "./ProjectCard";

function fullProject(): ProjectSummary {
  return {
    id: "p1",
    title: "Alpha",
    status: "active",
    // 2h before the faked "now" below -> "2h ago".
    updatedAt: "2026-07-09T10:00:00Z",
    rootPath: "/Users/user/Projects/Alpha",
    revision: "r1",
    path: "/some/path",
    taskCounts: {
      todo: 3,
      wip: 2,
      review: 1,
      blocked: 1,
      done: 5,
      total: 12,
    },
    gitStatus: { isGitRepo: false } as ProjectGitStatus,
    milestones: [{ id: "m1", title: "Ship v1", date: "2026-07-15" }],
  };
}

function minimalProject(): ProjectSummary {
  return {
    id: "p2",
    title: "Beta",
    status: "draft",
    updatedAt: "2026-07-09T12:00:00Z",
    rootPath: null,
    revision: "r2",
    path: "/some/other/path",
    taskCounts: {
      todo: 0,
      wip: 0,
      review: 0,
      blocked: 0,
      done: 0,
      total: 0,
    },
    gitStatus: { isGitRepo: false } as ProjectGitStatus,
  };
}

const noop = () => undefined;

function render(project: ProjectSummary): string {
  return renderToStaticMarkup(
    <ProjectCard
      project={project}
      stageLabel="Active"
      selected={false}
      agentActive={false}
      onSelect={noop}
    />,
  );
}

describe("ProjectCard", () => {
  it("renders all four self-explanatory lines for a full-featured project", () => {
    // Pin the system clock so relativeTime() is deterministic.
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-07-09T12:00:00Z"));

    const html = render(fullProject());

    // Line 1: title.
    expect(html).toContain("Alpha");
    // Line 2 (identity): folder basename + relative time.
    expect(html).toContain("Alpha"); // basename of /Users/user/Projects/Alpha
    expect(html).toContain("2h ago");
    // Line 3 (work state): compact task breakdown (rendered as separate spans
    // so `blocked` can be tinted, so assert the pieces individually).
    expect(html).toContain("2 wip");
    expect(html).toContain("1 review");
    expect(html).toContain("1 blocked");
    expect(html).toContain("5 done");
    // Line 4 (next milestone): diamond char + title + short date.
    expect(html).toContain("\u{25C7}");
    expect(html).toContain("Ship v1");
    expect(html).toContain("2026-07-15");

    // aria-label is enriched for screen readers: blocked count + next milestone.
    expect(html).toContain("1 blocked.");
    expect(html).toContain("Next milestone: Ship v1.");

    vi.useRealTimers();
  });

  it("surfaces the blocked count and overdue flag in the aria-label", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-07-09T12:00:00Z"));

    const project = {
      ...fullProject(),
      id: "p-blocked",
      title: "Delta",
      taskCounts: {
        todo: 0,
        wip: 0,
        review: 0,
        blocked: 3,
        done: 0,
        total: 3,
      },
      milestones: [{ id: "m-old", title: "Old thing", date: "2026-07-03" }],
    };
    const html = render(project);

    // Blocked-only breakdown renders "3 blocked" visually; aria-label mirrors it.
    expect(html).toContain("3 blocked.");
    // Milestone is overdue (today 2026-07-09 > 2026-07-03) -> label notes it.
    expect(html).toContain("Next milestone: Old thing (overdue).");

    vi.useRealTimers();
  });

  it("renders '<todo> to do' (not 'no tasks yet') when all tasks are pending todo", () => {
    const project = {
      ...fullProject(),
      id: "p-todo",
      title: "Gamma",
      taskCounts: {
        todo: 7,
        wip: 0,
        review: 0,
        blocked: 0,
        done: 0,
        total: 7,
      },
      milestones: undefined,
    };
    const html = render(project);

    expect(html).toContain("Gamma");
    // Factually-wrong "no tasks yet" must NOT appear when tasks exist.
    expect(html).not.toContain("no tasks yet");
    // The muted pending label should reference the 7 pending tasks.
    expect(html).toContain("7 to do");
  });

  it("renders the title + 'no tasks yet' and nothing else for a minimal project", () => {
    const html = render(minimalProject());

    expect(html).toContain("Beta");
    expect(html).toContain("no tasks yet");
    // No folder basename line (rootPath null) and no milestone line.
    expect(html).not.toContain("\u{25C7}");
    expect(html).not.toContain("2 wip");
  });
});
