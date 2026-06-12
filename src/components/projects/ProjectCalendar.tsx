// The Board calendar / organizer (Phase F of the Projects/Agents IA redesign).
//
// An agenda view that AGGREGATES milestones (deadlines) across ALL projects,
// grouped by date and sorted ascending. Rendered BELOW <ProjectsBoard> in Board
// mode only (ProjectsView mounts it in the unconditional Board render, which is
// skipped only in Work mode).
//
// All grouping/sorting/aggregation is in the pure, node-testable `projectCalendar`
// helper; this file is the thin JSX + the two mutate calls:
//   - add_project_milestone(projectId, title, date, note?)
//   - remove_project_milestone(projectId, milestoneId)
// Both go through invokeBackendCommand with camelCase args; on success the host's
// reload path (onChanged) refreshes the projects list so the agenda re-renders.
//
// Clicking a milestone selects/opens its project via the same handler the board
// cards use (onSelectProject). No secret/PII concern: milestones are user text;
// nothing here logs them.

import { useMemo, useRef, useState } from "react";
import { CalendarDays, Plus, Trash2 } from "lucide-react";
import { invokeBackendCommand } from "../../context/AppContext";
import type { ProjectSummary } from "../../types/backend";
import {
  addMilestoneArgs,
  formatDateHeading,
  groupMilestonesByDate,
  removeMilestoneArgs,
  totalMilestoneCount,
} from "./projectCalendarModel";

export interface ProjectCalendarProps {
  /** The Board's project list; milestones are aggregated across all of them. */
  projects: ReadonlyArray<ProjectSummary>;
  /** Select/open a project (same handler the board cards use). */
  onSelectProject: (projectId: string) => void;
  /** Called after a successful add/remove so the host reloads the projects. */
  onChanged: () => void;
}

export function ProjectCalendar({
  projects,
  onSelectProject,
  onChanged,
}: ProjectCalendarProps) {
  const groups = useMemo(() => groupMilestonesByDate(projects), [projects]);
  const total = useMemo(() => totalMilestoneCount(projects), [projects]);

  // Add-form state.
  const [showForm, setShowForm] = useState(false);
  const [formProjectId, setFormProjectId] = useState("");
  const [formTitle, setFormTitle] = useState("");
  const [formDate, setFormDate] = useState("");
  const [formNote, setFormNote] = useState("");
  const [error, setError] = useState<string | null>(null);
  // W5: the add and the removes are INDEPENDENT operations and must not gate each
  // other. The add has its own busy flag (disables only the Save button); a remove
  // tracks the in-flight milestone id so only THAT trash button is disabled while
  // its delete is in flight — an add and a remove (or two removes on different
  // milestones) can run concurrently without one freezing the other's UI. Each
  // operation keeps its own synchronous reentrancy guard against same-tick
  // double-clicks (a state flag alone cannot gate a same-tick double-click).
  const [addBusy, setAddBusy] = useState(false);
  const addBusyRef = useRef(false);
  const [removingId, setRemovingId] = useState<string | null>(null);
  // Set of milestone ids whose remove is in flight (synchronous double-click guard,
  // and supports concurrent removes of different milestones).
  const removingIdsRef = useRef<Set<string>>(new Set());

  // Only non-archived projects can take a new milestone target. The default
  // target follows the list so the select is never empty when projects exist.
  const targetableProjects = useMemo(
    () => projects.filter((project) => project.status !== "archived"),
    [projects],
  );

  const resetForm = () => {
    setFormProjectId("");
    setFormTitle("");
    setFormDate("");
    setFormNote("");
    setError(null);
  };

  const openForm = () => {
    setShowForm(true);
    setError(null);
    // Preselect the first targetable project so the select is never blank.
    if (!formProjectId && targetableProjects[0]) {
      setFormProjectId(targetableProjects[0].id);
    }
  };

  const closeForm = () => {
    setShowForm(false);
    resetForm();
  };

  const addMilestone = async () => {
    if (addBusyRef.current) return;
    const built = addMilestoneArgs({
      projectId: formProjectId,
      title: formTitle,
      date: formDate,
      note: formNote,
    });
    if (!built.ok) {
      setError(built.error);
      return;
    }
    addBusyRef.current = true;
    setAddBusy(true);
    setError(null);
    try {
      await invokeBackendCommand("add_project_milestone", built.args);
      closeForm();
      onChanged();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not add milestone.");
    } finally {
      addBusyRef.current = false;
      setAddBusy(false);
    }
  };

  const removeMilestone = async (projectId: string, milestoneId: string) => {
    // Per-milestone double-submit guard: a remove already in flight for THIS id is
    // ignored, but a remove of a different milestone (or an add) is not blocked.
    if (removingIdsRef.current.has(milestoneId)) return;
    removingIdsRef.current.add(milestoneId);
    setRemovingId(milestoneId);
    setError(null);
    try {
      await invokeBackendCommand(
        "remove_project_milestone",
        removeMilestoneArgs(projectId, milestoneId),
      );
      onChanged();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not remove milestone.");
    } finally {
      removingIdsRef.current.delete(milestoneId);
      // Reflect the latest in-flight id (or clear) for the disabled-state render.
      const remaining = removingIdsRef.current;
      setRemovingId(remaining.size > 0 ? [...remaining][remaining.size - 1] : null);
    }
  };

  return (
    <section
      data-help-title="The calendar lists every project deadline in one place."
      data-help-lines="Milestones are dates on a project, stored in its Markdown file and readable by Oracle.|Use them for deadlines, releases, or review gates across all your projects.|Click a milestone to open its project.|Add or remove milestones here; they persist immediately."
      className="rounded-2xl border border-cream-200 bg-white p-5"
    >
      <header className="mb-4 flex items-center justify-between gap-3">
        <div className="flex items-center gap-3">
          <div className="flex h-10 w-10 items-center justify-center rounded-2xl bg-teal/10">
            <CalendarDays className="h-5 w-5 text-teal" />
          </div>
          <div>
            <h3 className="text-sm font-semibold text-cream-800">Calendar</h3>
            <p className="text-[12px] text-cream-500">
              Deadlines and milestones across every project.
            </p>
          </div>
        </div>
        <button
          type="button"
          onClick={() => (showForm ? closeForm() : openForm())}
          disabled={targetableProjects.length === 0}
          className="inline-flex items-center gap-2 rounded-2xl bg-terracotta px-3 py-2 text-[12px] font-semibold text-white disabled:opacity-60"
        >
          <Plus className="h-3.5 w-3.5" />
          {showForm ? "Close" : "Add milestone"}
        </button>
      </header>

      {error && (
        <div className="mb-4 rounded-2xl border border-coral/20 bg-coral/[0.04] px-4 py-3 text-[12px] font-medium text-coral-dark">
          {error}
        </div>
      )}

      {showForm && (
        <div className="mb-5 grid grid-cols-1 gap-2 rounded-2xl border border-cream-200 bg-cream-50 p-3 sm:grid-cols-2">
          <select
            aria-label="Milestone project"
            value={formProjectId}
            onChange={(event) => setFormProjectId(event.target.value)}
            className="rounded-2xl border border-cream-200 bg-white px-3 py-2 text-[12px] text-cream-700 outline-none focus:border-terracotta-200"
          >
            <option value="" disabled>
              Choose project…
            </option>
            {targetableProjects.map((project) => (
              <option key={project.id} value={project.id}>
                {project.title}
              </option>
            ))}
          </select>
          <input
            type="date"
            aria-label="Milestone date"
            value={formDate}
            onChange={(event) => setFormDate(event.target.value)}
            className="rounded-2xl border border-cream-200 bg-white px-3 py-2 text-[12px] text-cream-700 outline-none focus:border-terracotta-200"
          />
          <input
            aria-label="Milestone title"
            value={formTitle}
            onChange={(event) => setFormTitle(event.target.value)}
            placeholder="Milestone title"
            className="rounded-2xl border border-cream-200 bg-white px-3 py-2 text-[12px] text-cream-700 outline-none focus:border-terracotta-200 sm:col-span-2"
          />
          <input
            aria-label="Milestone note (optional)"
            value={formNote}
            onChange={(event) => setFormNote(event.target.value)}
            placeholder="Note (optional)"
            className="rounded-2xl border border-cream-200 bg-white px-3 py-2 text-[12px] text-cream-700 outline-none focus:border-terracotta-200 sm:col-span-2"
          />
          <div className="flex justify-end gap-2 sm:col-span-2">
            <button
              type="button"
              onClick={() => void addMilestone()}
              disabled={addBusy}
              className="inline-flex items-center gap-2 rounded-2xl bg-teal px-3 py-2 text-[12px] font-semibold text-white disabled:opacity-60"
            >
              Save milestone
            </button>
          </div>
        </div>
      )}

      {total === 0 ? (
        <div className="rounded-2xl border border-dashed border-cream-200 bg-cream-50 px-4 py-10 text-center">
          <p className="text-[13px] font-medium text-cream-600">
            No milestones yet
          </p>
          <p className="mt-1 text-[12px] text-cream-400">
            Add a deadline or release date to see it on the calendar.
          </p>
        </div>
      ) : (
        <ol className="space-y-5">
          {groups.map((group) => (
            <li key={group.date}>
              <div className="mb-2 text-[11px] font-semibold uppercase tracking-widest text-cream-500">
                {formatDateHeading(group.date)}
              </div>
              <ul className="space-y-2">
                {group.entries.map((entry) => (
                  <li
                    key={entry.key}
                    className="flex items-start justify-between gap-3 rounded-2xl border border-cream-200 bg-cream-50 px-4 py-3"
                  >
                    <button
                      type="button"
                      onClick={() => onSelectProject(entry.projectId)}
                      className="min-w-0 flex-1 text-left"
                    >
                      <div className="truncate text-[13px] font-semibold text-cream-800">
                        {entry.title}
                      </div>
                      <div className="mt-0.5 truncate text-[11px] font-medium text-teal-dark">
                        {entry.projectTitle}
                      </div>
                      {entry.note && (
                        <div className="mt-1 line-clamp-2 text-[12px] text-cream-500">
                          {entry.note}
                        </div>
                      )}
                    </button>
                    <button
                      type="button"
                      aria-label={`Remove milestone ${entry.title}`}
                      onClick={() =>
                        void removeMilestone(entry.projectId, entry.milestoneId)
                      }
                      disabled={removingId === entry.milestoneId}
                      className="shrink-0 rounded-2xl border border-cream-200 bg-white p-2 text-cream-400 hover:text-coral disabled:opacity-60"
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                    </button>
                  </li>
                ))}
              </ul>
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}
