// Pure, DOM-free model for the Board calendar/organizer (Phase F of the
// Projects/Agents IA redesign).
//
// ProjectCalendar.tsx stays a thin JSX mapper; all the aggregation/grouping/sort
// logic lives here so it is unit-testable in node (this repo's vitest env has no
// jsdom). Everything is total and defensive: missing/empty milestone arrays, a
// project with no milestones, or an unparseable date never throw.

import type { ProjectMilestone, ProjectSummary } from "../../types/backend";

/** A milestone flattened with the project it belongs to, ready to render. */
export interface CalendarEntry {
  /** Stable per-entry key: `${projectId}:${milestoneId}`. */
  key: string;
  projectId: string;
  projectTitle: string;
  milestoneId: string;
  title: string;
  /** ISO calendar date, `YYYY-MM-DD`. */
  date: string;
  note: string | null;
}

/** A date bucket: all entries that share the same `date`, in stable order. */
export interface CalendarDateGroup {
  /** The shared ISO date, `YYYY-MM-DD`. */
  date: string;
  entries: CalendarEntry[];
}

/** Strict `YYYY-MM-DD` shape check (4-digit year, 2-digit month/day). Mirrors the
 *  backend's `clean_milestone_date` contract so a hand-edited bad date is simply
 *  skipped here rather than rendered in a wrong bucket. Does not validate the
 *  calendar (the backend already rejects impossible dates on write); this only
 *  guards the display from a clearly malformed string. */
function isIsoDate(value: unknown): value is string {
  return typeof value === "string" && /^\d{4}-\d{2}-\d{2}$/.test(value);
}

/** Flatten the milestones of every project into a single, project-tagged list.
 *  Projects with no milestones contribute nothing. Entries with a missing/blank
 *  title or a malformed date are dropped (defensive against hand-edited files). */
export function flattenMilestones(
  projects: ReadonlyArray<ProjectSummary>,
): CalendarEntry[] {
  const out: CalendarEntry[] = [];
  for (const project of projects) {
    const milestones: ReadonlyArray<ProjectMilestone> = project.milestones ?? [];
    for (const milestone of milestones) {
      if (!milestone || typeof milestone.id !== "string") continue;
      const title =
        typeof milestone.title === "string" ? milestone.title.trim() : "";
      if (title.length === 0) continue;
      if (!isIsoDate(milestone.date)) continue;
      const note =
        typeof milestone.note === "string" && milestone.note.trim().length > 0
          ? milestone.note.trim()
          : null;
      out.push({
        key: `${project.id}:${milestone.id}`,
        projectId: project.id,
        projectTitle: project.title,
        milestoneId: milestone.id,
        title,
        date: milestone.date,
        note,
      });
    }
  }
  return out;
}

/** Deterministic within-bucket order: by project title, then milestone title,
 *  then milestone id (so two identically-titled entries never reorder between
 *  renders). */
function compareEntries(a: CalendarEntry, b: CalendarEntry): number {
  return (
    a.projectTitle.localeCompare(b.projectTitle) ||
    a.title.localeCompare(b.title) ||
    a.milestoneId.localeCompare(b.milestoneId)
  );
}

/** Group milestones from all projects by date, sorted by date ASCENDING (ISO
 *  date strings sort lexicographically === chronologically). Within each date the
 *  entries are in the stable `compareEntries` order. Empty input → empty array. */
export function groupMilestonesByDate(
  projects: ReadonlyArray<ProjectSummary>,
): CalendarDateGroup[] {
  const entries = flattenMilestones(projects);
  const byDate = new Map<string, CalendarEntry[]>();
  for (const entry of entries) {
    const bucket = byDate.get(entry.date);
    if (bucket) bucket.push(entry);
    else byDate.set(entry.date, [entry]);
  }
  return [...byDate.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([date, bucket]) => ({
      date,
      entries: bucket.sort(compareEntries),
    }));
}

/** Total number of milestones across all projects (post-validation), so the host
 *  can pick the empty state without re-flattening. */
export function totalMilestoneCount(
  projects: ReadonlyArray<ProjectSummary>,
): number {
  return flattenMilestones(projects).length;
}

// ---- IPC arg builders (camelCase, validated) --------------------------------

/** camelCase args for the `add_project_milestone` command, or an error string when
 *  the local form is incomplete (so the UI can surface it without a round-trip).
 *  A blank note is sent as `null` (omitted), matching the backend's optional field.
 *  Whitespace-only fields are treated as empty. */
export function addMilestoneArgs(input: {
  projectId: string;
  title: string;
  date: string;
  note?: string;
}):
  | {
      ok: true;
      args: { projectId: string; title: string; date: string; note: string | null };
    }
  | { ok: false; error: string } {
  const projectId = input.projectId.trim();
  const title = input.title.trim();
  const date = input.date.trim();
  const note = (input.note ?? "").trim();
  if (!projectId) return { ok: false, error: "Pick a project for the milestone." };
  if (!title) return { ok: false, error: "Milestone title is required." };
  if (!date) return { ok: false, error: "Milestone date is required." };
  // W8: strict `YYYY-MM-DD` shape check BEFORE the IPC round-trip so the user gets
  // immediate feedback on a malformed date instead of a backend error. Mirrors the
  // backend's `clean_milestone_date` contract (the backend still re-validates the
  // calendar/year-range on write — this is the cheap client-side guard).
  if (!isIsoDate(date)) {
    return { ok: false, error: "Milestone date must use YYYY-MM-DD." };
  }
  return {
    ok: true,
    args: { projectId, title, date, note: note.length > 0 ? note : null },
  };
}

/** camelCase args for the `remove_project_milestone` command. */
export function removeMilestoneArgs(
  projectId: string,
  milestoneId: string,
): { projectId: string; milestoneId: string } {
  return { projectId, milestoneId };
}

/** Human label for a date bucket header, e.g. "Wed, Jul 15, 2026". Falls back to
 *  the raw ISO string if it can't be parsed (never throws). Parses the date parts
 *  explicitly and builds a UTC date so the displayed day never shifts by timezone
 *  (a naive `new Date("2026-07-15")` is parsed as UTC midnight and can render as
 *  the previous day in negative-offset zones). */
export function formatDateHeading(date: string): string {
  if (!isIsoDate(date)) return date;
  const [year, month, day] = date.split("-").map((part) => Number(part));
  const dt = new Date(Date.UTC(year, month - 1, day));
  if (Number.isNaN(dt.getTime())) return date;
  return dt.toLocaleDateString("en-US", {
    weekday: "short",
    year: "numeric",
    month: "short",
    day: "numeric",
    timeZone: "UTC",
  });
}
