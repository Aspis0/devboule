// Pure, DOM-free derivations that turn a `ProjectSummary` into the compact,
// self-explanatory strings shown on a `ProjectCard` face. Kept out of the JSX
// component so the formatting is unit-testable in node (this repo runs project
// tests without jsdom).
//
// Everything here is a pure function of its inputs (no `Date.now()`, no random,
// no I/O) except where an injectable `now`/`today` is passed in for determinism.

import type { ProjectMilestone, ProjectTaskCounts } from "../../types/backend";

/** Local `YYYY-MM-DD` for a Date (lexicographically comparable with milestone
 *  dates, which are also `YYYY-MM-DD`). */
function toYmd(d: Date): string {
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
}

/** Folder basename from a project root path. Handles POSIX (`/`) and Windows
 *  (`\`) separators, trims trailing separators, and returns `null` for
 *  `null`/empty input (the card then renders no folder line). */
export function folderBasename(rootPath: string | null): string | null {
  if (!rootPath) return null;
  const trimmed = rootPath.replace(/[\\/]+$/, "");
  if (!trimmed) return null;
  const parts = trimmed.split(/[\\/]/).filter(Boolean);
  return parts.length ? parts[parts.length - 1] : null;
}

/** Fixed render order for the compact task breakdown. `todo` is intentionally
 *  omitted — it is the default/background state, not worth a slot on the card. */
export const TASK_BREAKDOWN_ORDER: ReadonlyArray<{
  key: keyof ProjectTaskCounts;
  label: string;
}> = [
  { key: "wip", label: "wip" },
  { key: "review", label: "review" },
  { key: "blocked", label: "blocked" },
  { key: "done", label: "done" },
];

/**
 * Compact task-state breakdown, e.g. `"2 wip · 1 review · 1 blocked · 5 done"`.
 * Shows ONLY non-zero states in the fixed order `wip, review, blocked, done`.
 *
 * Returns `null` when `counts.total === 0` so the component can render the
 * single muted `"no tasks yet"` placeholder itself (kept here as null rather
 * than returning the literal string so the model stays free of display/colour
 * concerns and the component owns the wording/colour).
 */
export function taskCountsLine(counts: ProjectTaskCounts): string | null {
  if (counts.total === 0) return null;
  const parts = TASK_BREAKDOWN_ORDER.filter((s) => counts[s.key] > 0).map(
    (s) => `${counts[s.key]} ${s.label}`,
  );
  return parts.length ? parts.join(" · ") : null;
}

export interface NextMilestone {
  title: string;
  date: string;
  /** True when this is the most recent OVERDUE milestone (nothing is upcoming). */
  overdue: boolean;
}

/**
 * The milestone a card should surface:
 *  - soonest `date >= today` (upcoming) wins, with `overdue: false`;
 *  - if none are upcoming, the MOST RECENT overdue one wins, with `overdue: true`;
 *  - `null` when there are no milestones at all.
 *
 * `today` is injectable for deterministic tests (the component passes `new Date()`).
 * Dates are `YYYY-MM-DD` strings and compare correctly lexicographically.
 */
export function nextMilestone(
  milestones: ProjectMilestone[] | undefined,
  today: Date,
): NextMilestone | null {
  if (!milestones || milestones.length === 0) return null;
  const todayStr = toYmd(today);

  const upcoming = milestones
    .filter((m) => m.date >= todayStr)
    .sort((a, b) => (a.date < b.date ? -1 : a.date > b.date ? 1 : 0));
  if (upcoming.length > 0) {
    return { title: upcoming[0].title, date: upcoming[0].date, overdue: false };
  }

  const overdue = milestones
    .filter((m) => m.date < todayStr)
    .sort((a, b) => (a.date < b.date ? -1 : a.date > b.date ? 1 : 0));
  if (overdue.length > 0) {
    const latest = overdue[overdue.length - 1];
    return { title: latest.title, date: latest.date, overdue: true };
  }

  return null;
}
