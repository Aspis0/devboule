import {
  Activity,
  Archive,
  Bot,
  PauseCircle,
  RefreshCw,
  ShieldCheck,
} from "lucide-react";
import type {
  AgentSession,
  ProjectDetail,
  ProjectTaskCounts,
} from "../../types/backend";
import { fileName, gitPolicyLabel, gitPolicyTone } from "./projectFormat";
import { useMemo } from "react";
import {
  cliBadge,
  formatHeartbeatAge,
  healthTone,
  healthWord,
  sessionAgeMs,
  sessionHealth,
} from "./agentLiveStatus";
import { useNow } from "../../hooks/useNow";

// The working AGENT line: who · CLI · live status. Answers "who is working it"
// at a glance. Renders nothing when no agent session is supplied.
function WorkingAgentLine({
  session,
  now,
}: {
  session: AgentSession;
  // Shared live clock so the header agent's age + health recompute between data
  // polls (#3).
  now: number;
}) {
  const health = sessionHealth(session, now);
  const tone = healthTone(health);
  const badge = cliBadge(session.client);
  return (
    <div className="flex flex-wrap items-center gap-2 rounded-lg border border-cream-200 bg-cream-50 px-3 py-2">
      <Bot className="h-4 w-4 shrink-0 text-cream-500" aria-hidden />
      <span className="text-[12px] font-semibold text-cream-800">
        {session.agentId}
      </span>
      <span className="text-[11px] capitalize text-cream-500">
        {session.role}
      </span>
      <span
        className={`rounded-md px-1.5 py-0.5 text-[10px] font-semibold ${badge.toneClass}`}
      >
        {badge.label}
      </span>
      <span className="ml-auto flex items-center gap-1.5">
        <span
          className={`h-2 w-2 rounded-full ${
            tone === "working"
              ? "bg-sage-dark"
              : tone === "idle"
                ? "bg-amber-dark"
                : "bg-coral-dark"
          }`}
          aria-hidden
        />
        <span
          className={`text-[11px] font-semibold ${
            tone === "working"
              ? "text-sage-dark"
              : tone === "idle"
                ? "text-amber-dark"
                : "text-coral-dark"
          }`}
        >
          {healthWord(health)}
        </span>
        <span className="text-[10px] text-cream-400">
          {formatHeartbeatAge(sessionAgeMs(session, now))}
        </span>
      </span>
    </div>
  );
}

const statusDotTone: Record<string, string> = {
  active: "bg-sage",
  paused: "bg-amber",
  done: "bg-teal",
  archived: "bg-cream-400",
};

const statusLabel: Record<string, string> = {
  active: "active",
  paused: "paused",
  done: "done",
  archived: "archived",
};

// Always-visible calm status row for the selected project: title + lifecycle
// status dot/label + stage badge, a thin progress bar with a one-line task
// summary, and the existing lifecycle actions. Presentational only — every
// action is delegated to the callbacks owned by ProjectsView.
export function ProjectStatusHeader({
  project,
  stageLabel,
  stageToneClass,
  taskCounts,
  isBusy,
  workingAgent,
  onReload,
  onRefreshLiveStatus,
  onPause,
  onResume,
  onArchive,
}: {
  project: ProjectDetail;
  stageLabel: string | null;
  stageToneClass: string | null;
  taskCounts: ProjectTaskCounts | null;
  isBusy: boolean;
  // The agent currently working this project (who · CLI · live status). Null
  // when nobody is working it.
  workingAgent?: AgentSession | null;
  onReload: () => void;
  onRefreshLiveStatus: () => void;
  onPause: () => void;
  onResume: () => void;
  onArchive: () => void;
}) {
  const status = project.metadata.status;
  // Live clock so the working-agent line's age + health recompute between data
  // polls (#3).
  const now = useNow();

  // Prefer the summary task counts (same source the macro board uses); fall back
  // to the detail task list when the summary is not loaded yet. Memoized (#14)
  // so the fallback reduce does not run on every render when taskCounts is null.
  const counts = useMemo<ProjectTaskCounts>(
    () =>
      taskCounts ??
      project.state.tasks.reduce<ProjectTaskCounts>(
        (acc, task) => {
          acc.total += 1;
          if (task.status === "todo") acc.todo += 1;
          else if (task.status === "wip") acc.wip += 1;
          else if (task.status === "review") acc.review += 1;
          else if (task.status === "blocked") acc.blocked += 1;
          else if (task.status === "done") acc.done += 1;
          return acc;
        },
        { todo: 0, wip: 0, review: 0, blocked: 0, done: 0, total: 0 },
      ),
    [taskCounts, project.state.tasks],
  );
  const donePercent =
    counts.total > 0 ? Math.round((counts.done / counts.total) * 100) : 0;

  return (
    <section className="rounded-lg border border-cream-200 bg-white p-4">
      {/* WHO is working it (agent line). Shown whenever the caller supplies the
          working-agent prop, so a project without a live agent still gets the
          calm "nobody is working it" hint. */}
      {workingAgent !== undefined && (
        <div className="mb-4 flex flex-col gap-2 border-b border-cream-200 pb-4">
          {workingAgent ? (
            <WorkingAgentLine session={workingAgent} now={now} />
          ) : (
            <div className="flex items-center gap-2 rounded-lg border border-dashed border-cream-200 bg-cream-50 px-3 py-2">
              <Bot className="h-4 w-4 shrink-0 text-cream-400" aria-hidden />
              <span className="text-[11px] text-cream-400">
                No agent is working this project right now.
              </span>
            </div>
          )}
        </div>
      )}

      <div className="flex flex-col gap-3 md:flex-row md:items-start md:justify-between">
        <div className="min-w-0">
          <div className="mb-2 flex flex-wrap items-center gap-2">
            <span
              className={`h-2.5 w-2.5 shrink-0 rounded-full ${
                statusDotTone[status] ?? "bg-cream-400"
              }`}
              aria-hidden
            />
            <h2 className="truncate text-xl font-semibold text-cream-900">
              {project.metadata.title}
            </h2>
            <span className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
              {statusLabel[status] ?? status}
            </span>
            {stageLabel && stageToneClass && (
              <span
                className={`rounded-md px-2 py-1 text-[10px] font-semibold uppercase ${stageToneClass}`}
              >
                {stageLabel}
              </span>
            )}
            <span
              className={`rounded-md px-2 py-1 text-[10px] font-semibold ${gitPolicyTone(project.gitStatus.policyStatus)}`}
            >
              {gitPolicyLabel(project.gitStatus.policyStatus)}
            </span>
          </div>

          <div className="max-w-md">
            <div className="h-1.5 w-full overflow-hidden rounded-full bg-cream-100">
              <div
                className="h-full rounded-full bg-sage transition-all"
                style={{ width: `${donePercent}%` }}
                aria-hidden
              />
            </div>
            <p className="mt-1.5 text-[11px] font-medium text-cream-500">
              {counts.done}/{counts.total} done &middot; {counts.review} review
              &middot; {counts.blocked} blocked
            </p>
          </div>

          <p
            className="mt-2 max-w-full truncate font-mono text-[11px] text-cream-400"
            title={project.metadata.rootPath ?? project.path}
          >
            root {project.metadata.rootPath || "not set"}
          </p>
        </div>

        <div className="flex flex-wrap gap-2">
          <button
            onClick={onReload}
            disabled={isBusy}
            data-help-title="This reloads the selected project from disk."
            data-help-lines="Reload reads the Markdown file again.|Use it after a CLI agent or another tool edits the project.|It does not change tasks by itself.|If the file changed while you edited, reload helps avoid overwriting new work."
            className="inline-flex items-center gap-2 rounded-lg border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-600 hover:text-terracotta disabled:opacity-60"
          >
            <RefreshCw className="h-3.5 w-3.5" />
            Reload
          </button>
          <button
            onClick={onRefreshLiveStatus}
            disabled={isBusy}
            data-help-title="This refreshes live cloud status linked to the project."
            data-help-lines="Live status checks provider resources mentioned by the project when possible.|It is meant to show whether Cloudflare or Scaleway resources still match the task state.|It should be a read operation, not a provider mutation.|Use it before launching verifier or closing work."
            className="inline-flex items-center gap-2 rounded-lg border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-600 hover:text-sage-dark disabled:opacity-60"
          >
            <ShieldCheck className="h-3.5 w-3.5" />
            Live status
          </button>
          {status === "active" ? (
            <button
              onClick={onPause}
              disabled={isBusy}
              data-help-title="This pauses the project lifecycle."
              data-help-lines="Paused means agents should not start new work from this project.|It updates local project Markdown, not provider infrastructure.|Use it when the goal is waiting for a decision or external key.|Resume makes it launchable again."
              className="inline-flex items-center gap-2 rounded-lg border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-600 hover:text-amber-dark disabled:opacity-60"
            >
              <PauseCircle className="h-3.5 w-3.5" />
              Pause
            </button>
          ) : status !== "done" ? (
            <button
              onClick={onResume}
              disabled={isBusy}
              data-help-title="This resumes an inactive project."
              data-help-lines="Active projects can launch agents and receive normal task updates.|It only changes the local project file.|Before resuming, check whether old tokens, roots, or assumptions expired.|Agents will read the current Markdown state through MCP."
              className="inline-flex items-center gap-2 rounded-lg border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-600 hover:text-teal disabled:opacity-60"
            >
              <Activity className="h-3.5 w-3.5" />
              Resume
            </button>
          ) : null}
          {status !== "archived" && status !== "done" && (
            <button
              onClick={onArchive}
              disabled={isBusy}
              data-help-title="This archives the project locally."
              data-help-lines="Archived projects are treated as inactive work history.|It does not delete the Markdown file or provider resources.|Use it only when you do not want agents to continue that goal.|Oracle can still find archived notes unless indexing rules exclude them."
              className="inline-flex items-center gap-2 rounded-lg border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-600 hover:text-coral-dark disabled:opacity-60"
            >
              <Archive className="h-3.5 w-3.5" />
              Archive
            </button>
          )}
        </div>
      </div>

      <p
        className="mt-3 max-w-full truncate border-t border-cream-200 pt-3 font-mono text-[11px] text-cream-400"
        title={project.path}
      >
        {fileName(project.path)}
      </p>
    </section>
  );
}
