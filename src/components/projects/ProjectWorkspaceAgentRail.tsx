// Work-mode left rail: the project's live agents as a COMPACT, SELECTABLE list
// (one selected at a time, marked with a ◀ active indicator), each agent's
// reported subagents listed small with a "- " prefix, and a "+ Launch" toggle
// that mounts the shared SpawnPanel (coder/verifier only) scoped to this project.
//
// This is a thin JSX shell over the pure railRows / subagentRailLabel model
// (projectWorkspaceModel.ts). It consumes the sessions ProjectsView already polls
// (sessionsByProject) — it adds NO new poller. Selecting an agent calls onSelect;
// the center terminal in ProjectWorkspace is keyed by that id so it remounts
// cleanly on switch.

import { memo, useMemo } from "react";
import { Plus, Users } from "lucide-react";
import { useNow } from "../../hooks/useNow";
import type { AgentRoleRule, AgentSession, ProjectTask } from "../../types/backend";
import {
  cliBadge,
  healthTone,
  healthWord,
  sessionHealth,
  type LiveStatusTone,
} from "./agentLiveStatus";
import {
  railRows,
  subagentRailLabel,
  type RailAgentRow,
} from "./projectWorkspaceModel";
import { SpawnPanel } from "../agents/SpawnPanel";
import type { SpawnLaunchInput, SpawnSelection } from "../agents/agentRowModel";

const dotClass: Record<LiveStatusTone, string> = {
  working: "bg-sage-dark",
  idle: "bg-amber-dark",
  stalled: "bg-coral-dark",
};
const wordClass: Record<LiveStatusTone, string> = {
  working: "text-sage-dark",
  idle: "text-amber-dark",
  stalled: "text-coral-dark",
};
const roleTone: Record<string, string> = {
  coder: "bg-teal/10 text-teal",
  verifier: "bg-sage/10 text-sage-dark",
};

export interface ProjectWorkspaceAgentRailProps {
  /** Live sessions ALREADY filtered to this project (sessionsByProject) — no new
   *  poll happens here. */
  sessions: AgentSession[];
  selectedAgentId: string | null;
  onSelectAgent: (agentId: string) => void;
  // SpawnPanel wiring, scoped to this project.
  projectId: string;
  projectTitle: string;
  tasks: ProjectTask[];
  projectActive: boolean;
  isBusy: boolean;
  launchMessage: string | null;
  rules?: AgentRoleRule[];
  customClients?: { id: string; label: string; command: string }[];
  // The configured local Devboule main-coder model (config.localCoderBackend.model),
  // surfaced by SpawnPanel when the "Local (Devboule)" CLI is selected. Optional.
  localCoderModel?: string | null;
  onLaunch: (input: SpawnLaunchInput) => void;
  onCopyPrompt: (selection: SpawnSelection) => void;
  // Whether the launcher panel is expanded.
  launcherOpen: boolean;
  onToggleLauncher: () => void;
}

export function ProjectWorkspaceAgentRail({
  sessions,
  selectedAgentId,
  onSelectAgent,
  projectId,
  projectTitle,
  tasks,
  projectActive,
  isBusy,
  launchMessage,
  rules = [],
  customClients = [],
  localCoderModel = null,
  onLaunch,
  onCopyPrompt,
  launcherOpen,
  onToggleLauncher,
}: ProjectWorkspaceAgentRailProps) {
  // Live clock so each row's status dot/age recomputes between polls.
  const now = useNow();
  // Memoize the pure row tree so the 10s useNow tick (and unrelated prop churn)
  // doesn't rebuild it: it only depends on the sessions snapshot + selection.
  const rows = useMemo(
    () => railRows(sessions, selectedAgentId),
    [sessions, selectedAgentId],
  );
  const sessionById = useMemo(
    () => new Map(sessions.map((s) => [s.agentId, s])),
    [sessions],
  );

  return (
    <aside
      className="flex w-full flex-col gap-3"
      data-help-title="These are the agents working this project."
      data-help-lines="Select an agent to bring its live terminal to the center.|Subagents the agent reported are listed small under it with a dash.|A mini-coder a coder spawned is nested under it with a MINI chip and is selectable.|Use + Launch to spawn a coder or verifier scoped to this project."
    >
      <div className="flex items-center justify-between">
        <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
          Agents
        </h3>
        <button
          type="button"
          onClick={onToggleLauncher}
          disabled={!projectActive}
          title={
            projectActive
              ? "Launch a coder or verifier for this project"
              : "Only active projects can launch agents."
          }
          className={`inline-flex items-center gap-1 rounded-md border px-2 py-0.5 text-[10px] font-semibold disabled:opacity-60 ${
            launcherOpen
              ? "border-terracotta bg-terracotta/10 text-terracotta"
              : "border-cream-200 bg-white text-cream-600 hover:text-terracotta"
          }`}
        >
          <Plus className="h-3 w-3" aria-hidden />
          Launch
        </button>
      </div>

      {rows.length === 0 ? (
        <p className="rounded-lg border border-dashed border-cream-200 bg-cream-50 p-3 text-[11px] text-cream-400">
          No agent working this project yet.
        </p>
      ) : (
        <ul className="space-y-1.5">
          {rows.map((row) => (
            <AgentRailRow
              key={row.agentId}
              row={row}
              sessionById={sessionById}
              now={now}
              onSelectAgent={onSelectAgent}
            />
          ))}
        </ul>
      )}

      {launcherOpen && (
        <SpawnPanel
          projects={[{ id: projectId, title: projectTitle }]}
          lockedProjectId={projectId}
          selectedProjectId={projectId}
          tasks={tasks}
          projectActive={projectActive}
          isBusy={isBusy}
          message={launchMessage}
          rules={rules}
          customClients={customClients}
          localCoderModel={localCoderModel}
          onLaunch={onLaunch}
          onCopyPrompt={onCopyPrompt}
        />
      )}
    </aside>
  );
}

// ---- one rail row (parent or mini child) -----------------------------------
//
// A single selectable agent button. Parent rows additionally render their
// label-only subagents (non-clickable info, as before) AND their mini-coder
// children (clickable, indented, indigo MINI chip) below the button. A mini child
// is the SAME selectable button with the MINI chip and no further nesting.

interface AgentRailRowProps {
  row: RailAgentRow;
  sessionById: Map<string, AgentSession>;
  now: number;
  onSelectAgent: (agentId: string) => void;
  /** True when this row is rendered as a nested mini child (drives indentation). */
  nested?: boolean;
}

// WARNING 2: memoized so the 10s `useNow` tick (which re-renders the rail container)
// does not re-render every row. Props are stable enough to benefit: `onSelectAgent`
// is the stable `setSelectedAgentId`, `sessionById` is memoized in the parent, `row`
// identity changes only when `railRows` recomputes (sessions/selection change), and
// `now` is referentially equal between ticks unless it actually advances. A row only
// re-renders when its own inputs change.
const AgentRailRow = memo(function AgentRailRow({
  row,
  sessionById,
  now,
  onSelectAgent,
  nested = false,
}: AgentRailRowProps) {
  const session = sessionById.get(row.agentId);
  const tone: LiveStatusTone = session
    ? healthTone(sessionHealth(session, now))
    : "stalled";
  const word = session ? healthWord(sessionHealth(session, now)) : "stalled";
  const cli = cliBadge(session?.client);

  return (
    <li className={nested ? "ml-3 border-l border-cream-200 pl-2" : undefined}>
      <button
        type="button"
        onClick={() => onSelectAgent(row.agentId)}
        aria-pressed={row.selected}
        data-help-title={
          row.isMini
            ? "This is a mini-coder a coder delegated a small task to."
            : "This is an agent working this project."
        }
        data-help-lines={
          row.isMini
            ? "A mini runs one short task in its own live terminal.|Select it to watch its terminal in the center.|It is nested under the coder that spawned it.|It reaps itself when the task finishes."
            : "Select an agent to bring its live terminal to the center.|Subagents it reported are listed small with a dash.|Mini-coders it spawned are nested below, selectable.|Only one terminal is shown at a time."
        }
        className={`w-full rounded-lg border px-2.5 py-2 text-left transition-colors ${
          row.selected
            ? "border-terracotta bg-terracotta/[0.06]"
            : "border-cream-200 bg-white hover:border-terracotta/40"
        }`}
      >
        <div className="flex items-center gap-1.5">
          {row.selected && (
            <span className="text-[10px] font-bold text-terracotta" aria-hidden>
              ◀
            </span>
          )}
          <span
            className={`h-2 w-2 shrink-0 rounded-full ${dotClass[tone]}`}
            aria-hidden
          />
          <span
            className={`text-[10px] font-semibold uppercase tracking-wide ${wordClass[tone]}`}
          >
            {word}
          </span>
          <span className="ml-auto truncate text-[11px] font-semibold text-cream-800">
            {row.agentId}
          </span>
        </div>
        <div className="mt-1 flex flex-wrap items-center gap-1">
          {row.isMini && (
            <span className="rounded-md bg-indigo/10 px-1.5 py-0.5 text-[9px] font-semibold uppercase tracking-wide text-indigo-dark">
              Mini
            </span>
          )}
          <span
            className={`rounded-md px-1.5 py-0.5 text-[9px] font-semibold capitalize ${
              roleTone[row.role] ?? "bg-white text-cream-500"
            }`}
          >
            {row.role}
          </span>
          {row.orchestratorBadge && (
            <span className="rounded-md bg-terracotta/10 px-1.5 py-0.5 text-[9px] font-semibold text-terracotta">
              Orchestrator
            </span>
          )}
          <span
            className={`rounded-md px-1.5 py-0.5 text-[9px] font-semibold ${cli.toneClass}`}
          >
            {cli.label}
          </span>
          {row.orphanedMini && (
            <span className="rounded-md bg-cream-100 px-1.5 py-0.5 text-[9px] font-semibold text-cream-500">
              orphaned mini
            </span>
          )}
        </div>
        {/* Label-only subagents (heartbeat-reported, NO PTY): small, "- " prefix,
            non-clickable info lines — distinct from the mini children below. */}
        {row.subagents.length > 0 && (
          <ul className="mt-1 space-y-0.5">
            {row.subagents.map((sub, i) => (
              <li
                key={`${sub.label}-${i}`}
                className="flex items-center gap-1 text-[9px] text-cream-500"
              >
                <Users className="h-2.5 w-2.5 shrink-0" aria-hidden />
                <span className="truncate">- {subagentRailLabel(sub)}</span>
              </li>
            ))}
          </ul>
        )}
      </button>

      {/* Mini-coder children: real selectable live-PTY sessions, indented. */}
      {row.miniChildren.length > 0 && (
        <ul className="mt-1.5 space-y-1.5">
          {row.miniChildren.map((child) => (
            <AgentRailRow
              key={child.agentId}
              row={child}
              sessionById={sessionById}
              now={now}
              onSelectAgent={onSelectAgent}
              nested
            />
          ))}
        </ul>
      )}
    </li>
  );
});

export default ProjectWorkspaceAgentRail;
