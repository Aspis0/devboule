import { Bot, Copy, Play, Plus, SquareTerminal } from "lucide-react";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import type {
  AgentClaim,
  AgentEvent,
  AgentSession,
  ProjectTask,
} from "../../types/backend";
import { useNow } from "../../hooks/useNow";
import { AgentRow } from "../agents/AgentRow";
import type { SpawnRole } from "../agents/roleDisplay";

// Step 1 "Role" is a CHOICE: exactly one role selected at a time. Each option
// carries a one-line description shown inline + as a title attr so the picker
// explains itself. Default selection is "coder".
//
// Phase B role merge: only coder/verifier are spawnable. A coder PLANS and CODES;
// "orchestrator" is no longer a launch choice (it is a derived badge).
const launchRoles: {
  id: SpawnRole;
  label: string;
  description: string;
}[] = [
  {
    id: "coder",
    label: "Coder",
    description: "Plans, edits code, and moves tasks toward Review.",
  },
  {
    id: "verifier",
    label: "Verifier",
    description: "Audits work read-only and decides when Done is justified.",
  },
];

// Default the launcher's task to the recommended NEXT task: a Review task is the
// natural verifier target, otherwise the first non-done task. This keeps the
// "which work" choice explicit without forcing the user to pick every time.
function recommendedTaskId(tasks: ProjectTask[]): string {
  const review = tasks.find((task) => task.status === "review");
  if (review) return review.id;
  const open = tasks.find((task) => task.status !== "done");
  return open?.id ?? "";
}

// The single "who is working on this project" panel. It owns ONLY the local UI
// state: selected role, selected task, and whether the launcher is expanded
// while agents are already active. All launch/copy/control/stop/recovery logic
// stays in the parent and is passed in as callbacks. The working vs. idle split
// is driven entirely by the live sessions passed down.
//
// Rows are the shared canonical AgentRow (same component the global Agents room
// uses), so the two surfaces can never drift. The launcher below is kept project
// -local (it predates SpawnPanel and its onLaunch(role,client,taskId) contract is
// wired tightly into ProjectsView); reusing SpawnPanel here would force the
// parent onto the new host/model launch contract — a deliberate follow-up, kept
// out of this phase to bound the diff.
export function ProjectAgentPanel({
  sessions,
  claims,
  events,
  tasks = [],
  canLaunch,
  launchTitle,
  isBusy,
  launchMessage,
  ptyAgents,
  openTerminals,
  onToggleTerminal,
  renderTerminal,
  onLaunch,
  onCopyPrompt,
  onOpenCli,
  onStop,
  onRecovery,
}: {
  sessions: AgentSession[];
  claims: AgentClaim[];
  events: AgentEvent[];
  tasks?: ProjectTask[];
  canLaunch: boolean;
  launchTitle: string;
  isBusy: boolean;
  launchMessage: string | null;
  // App-hosted PTY gating for the shared row's Terminal toggle. Optional: when
  // the parent does not provide them the toggle simply never shows (zero
  // regression for callers that have not wired the PTY list yet).
  ptyAgents?: Set<string>;
  openTerminals?: Set<string>;
  onToggleTerminal?: (agentId: string) => void;
  renderTerminal?: (agentId: string) => ReactNode;
  onLaunch: (
    role: SpawnRole,
    client: "codex" | "claude",
    taskId?: string,
  ) => void;
  onCopyPrompt: (role: SpawnRole, taskId?: string) => void;
  onOpenCli?: (agentId: string) => void;
  onStop?: (agentId: string) => void;
  onRecovery?: (session: AgentSession) => void;
}) {
  // The live signal that drives the panel is the project's sessions. Claims and
  // events still gate whether the launcher starts collapsed (an agent recently
  // touched this project) but the visible list is one row per session.
  const hasWorkingAgent = sessions.length > 0;
  const hasRecentActivity =
    hasWorkingAgent || claims.length > 0 || events.length > 0;

  // Shared live clock for every agent row (#3).
  const now = useNow();

  const [selectedRole, setSelectedRole] = useState<SpawnRole>("coder");
  const [selectedTaskId, setSelectedTaskId] = useState<string>(() =>
    recommendedTaskId(tasks),
  );

  // recommendedTaskId(tasks) runs once at mount; if tasks were empty then (a
  // common race when the panel mounts before the project detail arrives) the
  // default stays empty forever. Re-sync once, only while still unset, so a
  // later-arriving task list seeds the launcher's default (#17).
  useEffect(() => {
    if (selectedTaskId !== "") return;
    const recommended = recommendedTaskId(tasks);
    if (recommended) setSelectedTaskId(recommended);
  }, [tasks, selectedTaskId]);
  // While agents are active the launcher collapses behind one toggle so the
  // panel never shows a second always-open launch block.
  const [showLauncher, setShowLauncher] = useState(false);
  const launcherVisible = !hasRecentActivity || showLauncher;

  // Resolve a "T-1 / Title" label per task id so rows and the selector read the
  // task, not just an opaque id.
  const taskLabelById = useMemo(() => {
    const map = new Map<string, string>();
    for (const task of tasks) map.set(task.id, `${task.id} / ${task.title}`);
    return map;
  }, [tasks]);

  const launchableTasks = useMemo(
    () => tasks.filter((task) => task.status !== "done"),
    [tasks],
  );

  const taskIdArg = selectedTaskId || undefined;

  return (
    <section
      className="rounded-lg border border-cream-200 bg-white p-4"
      data-help-title="This block shows who is working this project right now."
      data-help-lines="Each working agent is one row: its CLI, live status, and heartbeat age.|For Aspis Bio, check it before spawning agents so work is not duplicated.|The launcher lets you pick the role AND the exact task the next agent works on.|Launch opens a Codex or Claude terminal at the project root with MCP config."
    >
      <div className="mb-3 flex items-center justify-between gap-2">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <Bot className="h-4 w-4 shrink-0 text-terracotta" />
            <p className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
              Agent
            </p>
          </div>
          <p className="mt-0.5 text-[11px] text-cream-500">
            Who's working this project right now
          </p>
        </div>
      </div>

      {hasWorkingAgent ? (
        // WORKING state: one shared row per live session.
        <div className="space-y-2">
          {sessions.map((session) => (
            <AgentRow
              key={session.agentId}
              session={session}
              now={now}
              taskLabel={
                session.currentTaskId
                  ? (taskLabelById.get(session.currentTaskId) ??
                    session.currentTaskId)
                  : null
              }
              claims={claims}
              events={events}
              hasAppTerminal={ptyAgents?.has(session.agentId) ?? false}
              terminalOpen={openTerminals?.has(session.agentId) ?? false}
              onToggleTerminal={onToggleTerminal}
              renderTerminal={
                renderTerminal
                  ? () => renderTerminal(session.agentId)
                  : undefined
              }
              // Project-scoped panel: no Polis deep-link button here (the global
              // Agents room owns that). External-console focus + stop + recovery
              // stay. AgentRow.rowActions hides Open CLI for app-hosted agents
              // (host==="app") and shows a relaunch hint when their PTY exited, so
              // we pass the handler unconditionally and let the row decide.
              onOpenCli={onOpenCli}
              onStop={onStop}
              onRecovery={onRecovery}
            />
          ))}
        </div>
      ) : (
        <p className="text-[12px] leading-5 text-cream-400">No agent working</p>
      )}

      {/* While agents are active, the launcher hides behind one small toggle so
          there is never a duplicate-looking second panel. */}
      {hasRecentActivity && !showLauncher && (
        <button
          type="button"
          onClick={() => setShowLauncher(true)}
          disabled={!canLaunch}
          title={launchTitle}
          className="mt-3 inline-flex items-center gap-1 rounded-md border border-dashed border-cream-200 px-2 py-1 text-[11px] font-semibold text-cream-500 hover:text-terracotta disabled:opacity-60"
        >
          <Plus className="h-3.5 w-3.5" aria-hidden />
          Launch another
        </button>
      )}

      {launcherVisible && (
        <div
          className={`rounded-lg border border-cream-200 bg-cream-50 p-3 ${
            hasRecentActivity ? "mt-3" : "mt-4"
          }`}
        >
          {/* Step 1 — Role: a subdued segmented control, exactly one chip
              highlighted. It is a selector, not an action. */}
          <p className="mb-1.5 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
            Role
          </p>
          <div
            className="mb-2 inline-flex rounded-lg border border-cream-200 bg-white p-0.5"
            role="radiogroup"
            aria-label="Agent role"
          >
            {launchRoles.map((role) => {
              const active = selectedRole === role.id;
              return (
                <button
                  key={role.id}
                  type="button"
                  role="radio"
                  aria-checked={active}
                  onClick={() => setSelectedRole(role.id)}
                  title={role.description}
                  className={`rounded-md px-2.5 py-1 text-[11px] font-semibold transition-colors ${
                    active
                      ? "bg-terracotta/10 text-terracotta"
                      : "text-cream-500 hover:text-cream-800"
                  }`}
                >
                  {role.label}
                </button>
              );
            })}
          </div>
          <p className="mb-3 text-[10px] leading-4 text-cream-400">
            {launchRoles.find((role) => role.id === selectedRole)?.description}
          </p>

          {/* Step 2 — Task: explicit "which work" choice. The prompt is
              task-specific, so the selected task id is threaded into launch. */}
          <p className="mb-1.5 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
            Task
          </p>
          {launchableTasks.length > 0 ? (
            <select
              value={selectedTaskId}
              onChange={(event) => setSelectedTaskId(event.target.value)}
              data-help-title="This chooses the exact task the next agent works on."
              data-help-lines="The agent prompt is task-specific, so this decides which work it claims.|Project-level means the agent picks the next task itself via MCP.|Coder targets Todo/WIP/Blocked; verifier targets Review/Blocked.|The selected task id is threaded into the launch and the MCP claim."
              className="mb-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-700 outline-none focus:border-terracotta/30"
            >
              <option value="">Project-level (agent picks next task)</option>
              {launchableTasks.map((task) => (
                <option key={task.id} value={task.id}>
                  {task.id} / {task.status} / {task.title}
                </option>
              ))}
            </select>
          ) : (
            <p className="mb-1 text-[10px] leading-4 text-cream-400">
              No open task; the agent will work at project level.
            </p>
          )}
          <p className="mb-3 text-[10px] leading-4 text-cream-500">
            Will work on:{" "}
            <span className="font-semibold text-cream-700">
              {selectedTaskId
                ? (taskLabelById.get(selectedTaskId) ?? selectedTaskId)
                : "the next task the agent picks"}
            </span>
          </p>

          {/* Step 3 — Launch: three DISTINCT actionable buttons, threaded with
              the selected role AND the selected task id. */}
          <p className="mb-1.5 text-[10px] font-semibold uppercase tracking-widest text-cream-400">
            Launch
          </p>
          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              onClick={() => onLaunch(selectedRole, "codex", taskIdArg)}
              disabled={isBusy || !canLaunch}
              title={launchTitle}
              data-help-title="This launches the selected role in Codex on the selected task."
              data-help-lines="The app opens a Codex terminal at the project root.|It passes the task-specific prompt and MCP settings so the agent can claim and report status.|The role decides which tools are available and what the agent may change.|Use verifier after coder before closing important work."
              className="inline-flex items-center gap-1.5 rounded-md bg-terracotta px-3 py-1.5 text-[12px] font-semibold text-white hover:bg-terracotta/90 disabled:opacity-60"
            >
              <Play className="h-3.5 w-3.5" aria-hidden />
              Open in Codex
            </button>
            <button
              type="button"
              onClick={() => onLaunch(selectedRole, "claude", taskIdArg)}
              disabled={isBusy || !canLaunch}
              title={launchTitle}
              data-help-title="This launches the selected role in Claude on the selected task."
              data-help-lines="The app opens a Claude terminal at the project root.|It uses the same role and task choice as Codex.|Claude still needs MCP access to update tasks automatically.|Keep Cloudflare work coordinated because Claude may also use provider tooling."
              className="inline-flex items-center gap-1.5 rounded-md bg-teal px-3 py-1.5 text-[12px] font-semibold text-white hover:bg-teal/90 disabled:opacity-60"
            >
              <Play className="h-3.5 w-3.5" aria-hidden />
              Open in Claude
            </button>
            <button
              type="button"
              onClick={() => onCopyPrompt(selectedRole, taskIdArg)}
              disabled={!canLaunch}
              title={launchTitle}
              data-help-title="This copies a manual prompt for the selected role and task."
              data-help-lines="Manual prompt copy is for terminals you open yourself.|The role/task prompt tells the agent how to read the project and report through MCP.|It does not start a process or inject token profiles by itself.|Prefer app launch when provider tokens or root setup matter."
              className="inline-flex items-center gap-1.5 rounded-md border border-cream-200 bg-white px-3 py-1.5 text-[12px] font-semibold text-cream-600 hover:text-terracotta disabled:opacity-60"
            >
              <Copy className="h-3.5 w-3.5" aria-hidden />
              Copy prompt
            </button>
          </div>

          <p className="mt-2 flex items-center gap-1 text-[10px] leading-4 text-cream-400">
            <SquareTerminal className="h-3 w-3 shrink-0" aria-hidden />
            CLI clients need the `aspis-management` MCP config before launch.
          </p>

          {hasRecentActivity && (
            <button
              type="button"
              onClick={() => setShowLauncher(false)}
              className="mt-2 text-[10px] font-semibold text-cream-400 hover:text-cream-600"
            >
              Close
            </button>
          )}

          {launchMessage && (
            <p className="mt-2 rounded-md bg-sage/10 px-2 py-1 text-[10px] font-semibold text-sage-dark">
              {launchMessage}
            </p>
          )}
        </div>
      )}
    </section>
  );
}
