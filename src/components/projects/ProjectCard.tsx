import { Bot } from "lucide-react";
import { memo } from "react";
import type {
  ProjectGitStatus,
  ProjectStatus,
  ProjectSummary,
} from "../../types/backend";
import { censorChipAria, censorChipLabel, gitChipModel } from "./censorCounts";
import {
  folderBasename,
  nextMilestone,
  TASK_BREAKDOWN_ORDER,
  taskCountsLine,
} from "./projectCardModel";
import { formatDate, relativeTime } from "./projectFormat";

const statusDotTone: Record<ProjectStatus, string> = {
  draft: "bg-cream-300",
  active: "bg-sage",
  paused: "bg-amber",
  done: "bg-teal",
  archived: "bg-cream-400",
};

// The minimal project card FACE answers "the 3 questions" PLUS two at-a-glance
// chips added in the Projects/Agents IA redesign (Phase C):
//   WHAT  -> status dot + project name.
//   WHO   -> a single Bot with the working agent id/role, only when one is
//            actively working the project.
//   PROGRESS -> a compact done/total hint.
//   CHIPS -> a git chip (↑/↓/∆) and a Censor chip (⚠N) on the WHO line.
// The redesign deliberately REVERSES the prior rule that git/Censor badges lived
// only in the detail "BACK": the operator needs to see repo drift and open
// review findings without opening the card. The chips render COUNTS ONLY (never a
// branch name, commit, path, or any raw value) and HIDE entirely when there is
// nothing to show (clean repo / not a repo / zero findings). Everything else
// (per-status counts, claim/session lists, event snippets, root path) still
// lives in the detail BACK. Selection is unchanged: the whole card is a button.
//
// Wrapped in React.memo so a count change for ONE project (a new
// `censorCountByProject` reference, but identical entries for every other card)
// only re-renders the card whose `censorCount`/`gitStatus` actually changed —
// not all N cards. For memo to bite, the parent must pass STABLE props: in
// particular `onSelect` is `(projectId: string) => void` (a single stable
// callback shared by every card) rather than a per-card inline closure, so the
// card constructs its own click handler internally from the stable callback.
function ProjectCardComponent({
  project,
  stageLabel,
  selected,
  agentActive,
  agentLabel,
  gitStatus,
  censorCount,
  onSelect,
}: {
  project: ProjectSummary;
  stageLabel: string;
  selected: boolean;
  agentActive: boolean;
  /** Working agent id/role to show on the WHO line (only when agentActive). */
  agentLabel?: string | null;
  /** Repo status for the git chip; chip hides when not-a-repo or all-zero/clean. */
  gitStatus?: ProjectGitStatus | null;
  /** Open Censor finding count for the ⚠ chip; chip hides at 0/undefined. */
  censorCount?: number;
  /** Stable selection callback (receives this card's project id). */
  onSelect: (projectId: string) => void;
}) {
  const status = project.status;
  const counts = project.taskCounts;
  const who = agentActive ? (agentLabel ?? "agent at work") : null;
  const git = gitChipModel(gitStatus);
  const censorLabel = censorChipLabel(censorCount);
  const censorAria = censorChipAria(censorCount);

  // Self-explanatory face derivations (pure; see projectCardModel / projectFormat).
  const folder = folderBasename(project.rootPath);
  const relTime = relativeTime(project.updatedAt);
  const countsLine = taskCountsLine(counts);
  const next = nextMilestone(project.milestones, new Date());

  return (
    <button
      type="button"
      onClick={() => onSelect(project.id)}
      data-help-title="This opens a project from the stage board."
      data-help-lines="The stage board is the mini-Notion view of all projects.|Cards move stage based on tasks, agent claims, sessions, and verifier status.|Opening a card shows the detailed Kanban, notes, root, and agent controls.|The card itself does not modify the project."
      aria-label={`Open project ${project.title}. Stage ${stageLabel}. ${counts.done} of ${counts.total} tasks done.${counts.blocked > 0 ? ` ${counts.blocked} blocked.` : ""}${agentActive ? " Agent working." : ""}${next ? ` Next milestone: ${next.title}${next.overdue ? " (overdue)" : ""}.` : ""}`}
      aria-pressed={selected}
      className={`w-full rounded-lg border bg-white p-3 text-left shadow-soft-sm transition focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-terracotta/40 ${
        selected
          ? "border-terracotta/30"
          : "border-cream-200 hover:border-terracotta/20"
      }`}
    >
      {/* WHAT: status dot + project name. */}
      <div className="flex items-start gap-2">
        <span
          className={`mt-1.5 h-2 w-2 shrink-0 rounded-full ${
            statusDotTone[status] ?? "bg-cream-400"
          }`}
          aria-hidden
        />
        <p className="min-w-0 break-words text-[12px] font-semibold leading-5 text-cream-800">
          {project.title}
        </p>
      </div>

      {/* WHERE + WHEN: the project's folder basename (full path on hover) on the
          left, a coarse relative updated-at on the right. Both single-line,
          truncated, so the card never grows unpredictably tall. */}
      <div className="mt-1.5 flex items-center justify-between gap-2 pl-4">
        {folder ? (
          <span
            className="min-w-0 truncate font-mono text-[10px] text-cream-500"
            title={project.rootPath ?? undefined}
          >
            {folder}
          </span>
        ) : (
          <span />
        )}
        <span className="shrink-0 font-mono text-[10px] text-cream-400">
          {relTime}
        </span>
      </div>

      {/* WHO + CHIPS + PROGRESS: a single muted line. WHO on the left; the
          git/Censor chips and the compact per-state breakdown clustered right.
          `blocked` is tinted the warn/coral tone used elsewhere for warnings. */}
      <div className="mt-2 flex items-center justify-between gap-2 pl-4">
        {who ? (
          <span
            className="inline-flex min-w-0 items-center gap-1 text-cream-500"
            title={`Agent working: ${who}`}
          >
            <Bot className="h-3 w-3 shrink-0 text-terracotta" aria-hidden />
            <span className="truncate text-[10px] font-medium">{who}</span>
          </span>
        ) : (
          <span />
        )}
        <span className="flex shrink-0 items-center gap-1.5">
          {/* Git chip: repo drift at a glance (↑ahead ↓behind N∆ dirty). Counts
              only — never the branch/commit. Hidden when clean or not a repo. */}
          {git && (
            <span
              aria-label={git.ariaLabel}
              title={git.ariaLabel}
              className="inline-flex items-center gap-1 rounded-md bg-teal/10 px-1.5 py-0.5 font-mono text-[9px] font-semibold leading-none text-teal-dark"
            >
              {git.segments.map((segment) => (
                <span key={segment}>{segment}</span>
              ))}
            </span>
          )}
          {/* Censor chip: open automated-review findings (⚠N). Count only.
              Hidden at 0/undefined. */}
          {censorLabel && censorAria && (
            <span
              aria-label={censorAria}
              title={censorAria}
              data-help-title="Open Censor findings for this project."
              data-help-lines="Censor is the automated, local-first code reviewer that watches files as agents write them.|This count is the number of OPEN findings (linters, smells, secrets) not yet fixed or dismissed.|It updates live as agents work; open the project to triage them.|A zero count (no chip) means nothing is currently flagged."
              className="inline-flex items-center rounded-md bg-coral/10 px-1.5 py-0.5 font-mono text-[9px] font-semibold leading-none text-coral-dark"
            >
              {censorLabel}
            </span>
          )}
          {countsLine ? (
            <span className="flex shrink-0 items-center gap-1.5 font-mono text-[10px] font-semibold">
              {TASK_BREAKDOWN_ORDER.filter((s) => counts[s.key] > 0).map(
                (s, i) => (
                  <span key={s.key} className="flex items-center gap-1.5">
                    {i > 0 && (
                      <span className="text-cream-300" aria-hidden>
                        ·
                      </span>
                    )}
                    <span
                      className={
                        s.key === "blocked"
                          ? "text-coral-dark"
                          : "text-cream-400"
                      }
                    >
                      {counts[s.key]} {s.label}
                    </span>
                  </span>
                ),
              )}
            </span>
          ) : counts.total === 0 ? (
            <span className="shrink-0 font-mono text-[10px] font-medium text-cream-400">
              no tasks yet
            </span>
          ) : (
            <span className="shrink-0 font-mono text-[10px] font-medium text-cream-400">
              {counts.todo} to do
            </span>
          )}
        </span>
      </div>

      {/* NEXT MILESTONE (conditional): the soonest upcoming milestone, or the
          most recent overdue one (coral + "overdue"). Omitted entirely when the
          project has no milestones. Single-line + truncate. */}
      {next && (
        <div className="mt-1 flex min-w-0 items-center gap-1 pl-4">
          <span className="shrink-0 text-cream-400" aria-hidden>
            {"\u{25C7}"}
          </span>
          <span
            className={`min-w-0 truncate text-[10px] ${
              next.overdue ? "text-coral-dark" : "text-cream-500"
            }`}
          >
            {next.title}
          </span>
          <span
            className={`shrink-0 font-mono text-[10px] ${
              next.overdue ? "text-coral-dark" : "text-cream-400"
            }`}
          >
            {formatDate(next.date)}
          </span>
          {next.overdue && (
            <span className="shrink-0 text-[10px] font-medium text-coral-dark">
              overdue
            </span>
          )}
        </div>
      )}
    </button>
  );
}

// Memoized: the board passes a brand-new `censorCountByProject` object whenever
// ANY project's count changes, but the per-card scalar/object props (censorCount,
// gitStatus, selected, agentActive, agentLabel, stageLabel, project, the stable
// onSelect) are unchanged for every other card, so default shallow prop equality
// skips re-rendering them. Only the card whose props actually changed re-renders.
export const ProjectCard = memo(ProjectCardComponent);
