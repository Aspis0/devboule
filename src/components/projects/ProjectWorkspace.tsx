// Work-mode full-screen IDE shell (Phase D). Rendered full-bleed by ProjectsView
// when `workMode && currentProject`; the kanban / calendar chrome is skipped
// (the per-project task board + Notes now live HERE, via slots). Layout:
//   - Top bar: ← Board, project name, git status (from project.gitStatus) +
//     [Commit]/[Push] wired to the new backend commands.
//   - Left rail: ProjectWorkspaceAgentRail (the project's agents, selectable).
//   - Center: the SELECTED agent's live terminal (AgentTerminalViewer, lazy
//     chunk, KEYED by agentId so switching agents remounts cleanly) + a
//     [drawer ▸] control opening AgentDetailDrawer for that agent.
//   - Task board (taskBoardSlot) above the dock; bottom dock: Censor (default)
//     / Git / Plans / Console / MCP; Notes (notesSlot) below the dock.
//
// CRITICAL: this component adds NO agent-state poller. It consumes the sessions /
// claims / events ProjectsView already polls (passed as props). The terminal is
// the SAME lazy AgentTerminalViewer the Agents room uses, mounted ONE at a time.

import {
  ArrowLeft,
  GitBranch,
  LifeBuoy,
  Minimize2,
  OctagonX,
  PanelRightOpen,
  Square,
  Terminal,
} from "lucide-react";
import {
  Suspense,
  lazy,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { invokeBackendCommand } from "../../context/AppContext";
import { useNow } from "../../hooks/useNow";
import type {
  AgentClaim,
  AgentEvent,
  AgentRoleRule,
  AgentSession,
  ProjectDetail,
} from "../../types/backend";
import type { CustomAgentClient } from "../../types/config";
import type { SpawnLaunchInput, SpawnSelection } from "../agents/agentRowModel";
import { AgentConsole } from "../agents/AgentConsole";
import { AgentTimelineStrip } from "../agents/AgentTimelineStrip";
import { consoleRunCount } from "../agents/agentConsoleModel";
import { useAgentConsole } from "../agents/useAgentConsole";
import { AgentDetailDrawer } from "../agents/AgentDetailDrawer";
import { AgentQuestionCard } from "./AgentQuestionCard";
import { CensorPanel } from "./CensorPanel";
import { MiniSteerBar } from "./MiniSteerBar";
import { PlanApprovalCard } from "./PlanApprovalCard";
import { PlansDockTab } from "./PlansPanel";
import { ProjectWorkspaceAgentRail } from "./ProjectWorkspaceAgentRail";
import { PushApprovalCard } from "./PushApprovalCard";
import { ProjectMcpServersCard } from "./ProjectMcpServersCard";
import { ChangesDockTab } from "./ChangesDockTab";
import {
  DEFAULT_DOCK_TAB,
  DOCK_TABS,
  type DockTab,
  compactWriteCall,
  isMiniSession,
  miniKillCall,
  reconcileSelectedAgentId,
  shouldShowCompact,
  workspaceGitLine,
} from "./projectWorkspaceModel";
import { TokenUsageBadge } from "../agents/TokenUsageBadge";
import { useAgentTokenUsage } from "../agents/useAgentTokenUsage";
import type { AgentTokenUsage } from "../../types/backend";

// Same lazy xterm chunk the Agents room + ProjectsView board use; loaded once.
const AgentTerminalViewer = lazy(() =>
  import("../agents/AgentTerminalViewer").then((m) => ({
    default: m.AgentTerminalViewer,
  })),
);

export interface ProjectWorkspaceProps {
  project: ProjectDetail;
  /** Sessions for THIS project (sessionsByProject) — consumed, not re-polled. */
  sessions: AgentSession[];
  /** Open claims for this project (from the same board state). */
  claims: AgentClaim[];
  /** Recent events for this project (from the same board state). */
  events: AgentEvent[];
  /** agent_ids with a live app-hosted PTY (from the board's agent_pty_list).
   *  Gates the center terminal: an agent without an app PTY (external/legacy) gets
   *  a tidy explanatory state instead of the viewer's error banner. */
  ptyAgents: Set<string>;
  isBusy: boolean;
  canLaunch: boolean;
  launchMessage: string | null;
  rules?: AgentRoleRule[];
  customClients?: CustomAgentClient[];
  /** The configured local Devboule main-coder model (config.localCoderBackend.model),
   *  threaded to SpawnPanel via the agent rail. Optional. */
  localCoderModel?: string | null;
  onBack: () => void;
  onLaunch: (input: SpawnLaunchInput) => void;
  onCopyPrompt: (selection: SpawnSelection) => void;
  onCommit: (message: string) => void;
  onPush: () => void;
  onPull: () => void;
  /** Stop a NORMAL (non-mini) agent: kills its process tree via the backend's
   *  `stop_agent` flow (the same one the removed board-mode panel used). The mini
   *  safety brake (`mini_coder_kill`) is a SEPARATE, in-component path — the two
   *  never apply to the same selected agent. */
  onStopAgent: (agentId: string) => void;
  /** Focus the dedicated external-console window of an agent that runs OUTSIDE the
   *  app (no in-app PTY). Backend `focus_agent_terminal`. */
  onFocusCli: (agentId: string) => void;
  /** Copy a client-side recovery prompt for the selected agent (no backend call,
   *  no secret). The parent resolves the live session from this agent id. */
  onCopyRecovery: (agentId: string) => void;
  /** Inline status message for the last commit/push/pull (success or git stderr). */
  gitActionMessage: string | null;
  gitActionError: boolean;
  gitActionBusy: boolean;
  /** The per-project task board, built by ProjectsView (reusing its handlers) and
   *  rendered here ABOVE the dock tabs. Optional → absent renders nothing. */
  taskBoardSlot?: ReactNode;
  /** The per-project Notes section, built by ProjectsView, rendered BELOW the dock. */
  notesSlot?: ReactNode;
}

export function ProjectWorkspace({
  project,
  sessions,
  claims,
  events,
  ptyAgents,
  isBusy,
  canLaunch,
  launchMessage,
  rules = [],
  customClients = [],
  localCoderModel = null,
  onBack,
  onLaunch,
  onCopyPrompt,
  onCommit,
  onPush,
  onPull,
  onStopAgent,
  onFocusCli,
  onCopyRecovery,
  gitActionMessage,
  gitActionError,
  gitActionBusy,
  taskBoardSlot,
  notesSlot,
}: ProjectWorkspaceProps) {
  const now = useNow();
  const [selectedAgentId, setSelectedAgentId] = useState<string | null>(() =>
    reconcileSelectedAgentId(null, sessions),
  );
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [dockTab, setDockTab] = useState<DockTab>(DEFAULT_DOCK_TAB);
  const [launcherOpen, setLauncherOpen] = useState(false);
  const [commitOpen, setCommitOpen] = useState(false);
  const [commitMessage, setCommitMessage] = useState("");
  const [costTotalUsd, setCostTotalUsd] = useState<number | null>(null);

  // The previous sessions snapshot, so reconcile can tell whether a now-gone
  // selection WAS a mini (and thus fall back to its parent, not the freshest).
  const prevSessionsRef = useRef<AgentSession[]>(sessions);

  // Reconcile the selection against the live session list every render-relevant
  // change: keep a still-live selection, otherwise fall back — to the gone mini's
  // PARENT when it was a mini and the parent survives, else the freshest survivor
  // (or null). This prunes a dangling selection when the selected agent exits — no
  // second poller, just a derive off the props ProjectsView feeds.
  useEffect(() => {
    // BLOCKER 2: capture the genuinely-prior snapshot, then update the ref to the new
    // sessions BEFORE calling setSelectedAgentId. The setState updater runs LATER
    // (async), so if we updated the ref after it, the updater would read the
    // already-overwritten ref (prev === current) and a reaped mini would never fall
    // back to its parent. Closing over the captured `prev` makes reconcile see the
    // real prior list.
    const prev = prevSessionsRef.current;
    prevSessionsRef.current = sessions;
    setSelectedAgentId((current) =>
      reconcileSelectedAgentId(current, sessions, prev),
    );
  }, [sessions]);

  const gitLine = useMemo(
    () => workspaceGitLine(project.gitStatus),
    [project.gitStatus],
  );

  const selectedSession = useMemo<AgentSession | null>(
    () => sessions.find((s) => s.agentId === selectedAgentId) ?? null,
    [sessions, selectedAgentId],
  );

  // MC-P5: the Stop (kill) safety brake is gated to a SELECTED mini session only —
  // a mini is a session with a parentAgentId. A normal agent's stop is the
  // separate `stop_agent` flow wired through onStopAgent below, NOT this 1-click
  // brake.
  const selectedIsMini = useMemo(
    () => (selectedSession ? isMiniSession(selectedSession) : false),
    [selectedSession],
  );

  // The Stop control for a NORMAL agent (the restored board-mode `stop_agent`
  // capability): shown ONLY for a selected NON-mini agent with a live session.
  // Mutually exclusive with the mini brake above — a mini shows the mini brake,
  // a normal agent shows this. Either, but never both, on the same selection.
  const selectedShowsNormalStop = Boolean(selectedSession) && !selectedIsMini;

  // Open-CLI is meaningful ONLY for an agent that runs in an EXTERNAL console
  // (no in-app PTY). This mirrors the same condition that renders the
  // "runs in an external console" placeholder below: a non-app host with no live
  // in-app PTY. An app-hosted agent whose PTY exited has no external window to
  // focus, so it stays hidden (matching agentRowModel.rowActions.showOpenCli).
  const selectedShowsOpenCli =
    selectedSession != null &&
    selectedSession.host !== "app" &&
    !ptyAgents.has(selectedSession.agentId);

  // MC-P7: the Compact action is gated to a SELECTED session whose RESOLVED
  // built-in client is exactly "claude" — `/compact` is a Claude Code slash
  // command, meaningless to codex/powershell/ollama-mini/custom CLIs. It is an
  // independent control from the mini Stop brake (Stop = mini-only safety kill;
  // Compact = claude-only context hygiene); either, both, or neither may show
  // depending on the selected session.
  const selectedCanCompact = useMemo(
    () => (selectedSession ? shouldShowCompact(selectedSession) : false),
    [selectedSession],
  );

  // MC-P6: token/cost window for the SELECTED agent ONLY, on a slow lazy cadence
  // (transcript reads are expensive — never per rail row, never on the 5s
  // live-state tick). Degrades silently to a hidden badge on unavailable.
  const tokenUsage = useAgentTokenUsage(selectedAgentId, {
    fetchUsage: (agentId) =>
      invokeBackendCommand<AgentTokenUsage>("get_agent_token_usage", {
        agentId,
      }),
  });

  // The structured Console timeline for the SELECTED agent. The hook degrades to
  // the empty resting state until the Step B backend (mini_activity_snapshot +
  // mini-activity://<agentId>) lands — no second poller, no GPU. Its `running`
  // flag drives the spinner + run-count pill on the Console tab.
  const consoleActivity = useAgentConsole(selectedAgentId);
  const consoleRunning = Boolean(consoleActivity.running);
  const consoleCount = consoleRunCount(consoleActivity);

  // P2 cost: fetch running cost total on mount and whenever a new task estimate arrives.
  useEffect(() => {
    let cancelled = false;
    invokeBackendCommand<{ totalUsd: number; byModel: Record<string, number> }>(
      "get_cost_summary"
    )
      .then((summary) => {
        if (!cancelled) setCostTotalUsd(summary.totalUsd);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [consoleActivity?.taskCostEstimateUsd]);

  // MC-P5: 1-click kill of the selected mini-coder. It is a TRUE safety brake (no
  // two-step confirm): `mini_coder_kill` records killRequested THEN kills the PTY so
  // the executor finalizes the mini as aborted_by_human and the parent coder is told
  // to escalate. Best-effort; a failed invoke is swallowed (the executor's own
  // timeout/parent-gone backstops still reap a runaway mini).
  const stopMini = useCallback(() => {
    if (!selectedSession) return;
    const call = miniKillCall(selectedSession);
    if (!call) return; // not a mini — no 1-click kill for a normal agent.
    void invokeBackendCommand(call.command, call.args).catch(() => {
      /* swallow — the executor backstops a runaway mini regardless */
    });
  }, [selectedSession]);

  // MC-P7: run `/compact` in the selected Claude agent's terminal. Reuses the
  // EXISTING `agent_pty_write` path (the same the reply bar uses) to write the
  // fixed `/compact\n` literal — no new write path, no secret. Gated by
  // `compactWriteCall` returning null for any non-claude session, so a stray call
  // on a wrong client can never fire. Best-effort; a failed invoke is swallowed.
  const compactSelected = useCallback(() => {
    if (!selectedSession) return;
    const call = compactWriteCall(selectedSession);
    if (!call) return; // not a claude client — no Compact for it.
    void invokeBackendCommand(call.command, call.args).catch(() => {
      /* swallow — Compact is a convenience; a write failure is non-fatal */
    });
  }, [selectedSession]);

  // Stop a NORMAL (non-mini) selected agent via the parent's restored
  // `stop_agent` flow. Gated to a non-mini selection (the mini brake owns minis),
  // so a mistaken call on a mini can never fire.
  const stopSelected = useCallback(() => {
    if (!selectedSession || isMiniSession(selectedSession)) return;
    onStopAgent(selectedSession.agentId);
  }, [selectedSession, onStopAgent]);

  // Focus the selected agent's external console window (parent's
  // `focus_agent_terminal`). The button is only shown for an external-console
  // agent, but re-derive the condition here so it can never fire otherwise.
  const focusCliSelected = useCallback(() => {
    if (
      !selectedSession ||
      selectedSession.host === "app" ||
      ptyAgents.has(selectedSession.agentId)
    ) {
      return;
    }
    onFocusCli(selectedSession.agentId);
  }, [selectedSession, ptyAgents, onFocusCli]);

  // Copy a recovery prompt for the selected agent (parent builds the text from
  // the live session — pure, no backend call, no secret).
  const copyRecoverySelected = useCallback(() => {
    if (!selectedSession) return;
    onCopyRecovery(selectedSession.agentId);
  }, [selectedSession, onCopyRecovery]);

  const submitCommit = () => {
    const trimmed = commitMessage.trim();
    if (!trimmed || gitActionBusy) return;
    onCommit(trimmed);
    setCommitMessage("");
    setCommitOpen(false);
  };

  return (
    <div className="flex w-full flex-col gap-4">
      {/* ---- Top bar ---- */}
      <div className="flex flex-col gap-2 rounded-2xl border border-cream-200 bg-white p-3 lg:flex-row lg:items-center lg:justify-between">
        <div className="flex min-w-0 items-center gap-3">
          <button
            type="button"
            onClick={onBack}
            data-help-title="This returns to the project board."
            data-help-lines="Work mode is a full-screen view of one project.|Going back keeps this project selected on the board.|The agents keep running; you are only changing the view.|Use the board to switch between projects."
            className="inline-flex shrink-0 items-center gap-1.5 rounded-lg border border-cream-200 bg-white px-3 py-1.5 text-[12px] font-semibold text-cream-600 hover:text-terracotta"
          >
            <ArrowLeft className="h-3.5 w-3.5" aria-hidden />
            Board
          </button>
          <h2 className="truncate text-sm font-semibold text-cream-800">
            {project.metadata.title}
          </h2>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          {gitLine.isGitRepo && (
            <span
              className="inline-flex items-center gap-1.5 rounded-lg bg-cream-50 px-2.5 py-1 text-[11px] font-semibold text-cream-600"
              title={gitLine.segments.join(" · ")}
            >
              <GitBranch className="h-3.5 w-3.5 text-cream-400" aria-hidden />
              {gitLine.segments.join(" · ")}
            </span>
          )}
          <button
            type="button"
            onClick={onPull}
            disabled={!gitLine.isGitRepo || gitActionBusy}
            data-help-title="This pulls the latest changes from origin (fast-forward only)."
            data-help-lines="Pull downloads the current branch's new commits from origin and fast-forwards.|It never merges or rebases: if your branch has diverged, it stops and shows the git error.|Resolve a divergence yourself (commit/stash, then merge or rebase) before pulling again.|The working tree is left untouched when a fast-forward is not possible."
            className="inline-flex items-center gap-1.5 rounded-lg border border-cream-200 bg-white px-3 py-1.5 text-[12px] font-semibold text-cream-700 hover:bg-cream-50 disabled:opacity-60"
          >
            Pull
          </button>
          <button
            type="button"
            onClick={() => setCommitOpen((open) => !open)}
            disabled={!gitLine.isGitRepo || gitActionBusy}
            data-help-title="This commits the tracked changes on the current branch."
            data-help-lines="A commit records the modified, tracked files on the current branch only.|Untracked files are not swept in; stage them in your editor if needed.|Enter a short message describing the change.|The app never force-anything; on failure the git error is shown."
            className="inline-flex items-center gap-1.5 rounded-lg bg-terracotta px-3 py-1.5 text-[12px] font-semibold text-white hover:bg-terracotta/90 disabled:opacity-60"
          >
            Commit
          </button>
          <button
            type="button"
            onClick={onPush}
            disabled={!gitLine.isGitRepo || gitActionBusy}
            data-help-title="This pushes the current branch to origin."
            data-help-lines="Push uploads the current branch's commits to the origin remote.|It never force-pushes, so it can only fast-forward the remote.|If there is no upstream or the push is rejected, the git error is shown.|Commit first if you have local changes you want to push."
            className="inline-flex items-center gap-1.5 rounded-lg bg-teal px-3 py-1.5 text-[12px] font-semibold text-white hover:bg-teal/90 disabled:opacity-60"
          >
            Push
          </button>
        </div>
      </div>

      {/* Commit message input (small, inline — no modal). */}
      {commitOpen && (
        <div className="flex flex-col gap-2 rounded-2xl border border-cream-200 bg-white p-3 sm:flex-row sm:items-center">
          <input
            value={commitMessage}
            onChange={(e) => setCommitMessage(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") submitCommit();
            }}
            placeholder="Commit message"
            maxLength={2000}
            autoFocus
            className="min-w-0 flex-1 rounded-lg border border-cream-200 bg-cream-50 px-3 py-2 text-[12px] text-cream-700 outline-none focus:border-terracotta-200"
          />
          <button
            type="button"
            onClick={submitCommit}
            disabled={!commitMessage.trim() || gitActionBusy}
            className="shrink-0 rounded-lg bg-terracotta px-3 py-2 text-[12px] font-semibold text-white disabled:opacity-60"
          >
            Commit
          </button>
        </div>
      )}

      {/* Inline git action status (success or git stderr). */}
      {gitActionMessage && (
        <p
          className={`rounded-lg px-3 py-2 text-[11px] font-semibold ${
            gitActionError
              ? "bg-coral/[0.06] text-coral-dark"
              : "bg-sage/10 text-sage-dark"
          }`}
        >
          {gitActionMessage}
        </p>
      )}

      {/* GH-P4: agent push-approval gate — agents commit freely, the human approves
          every push. Surfaces this project's pending request(s) with Approve/Deny. */}
      <PushApprovalCard projectId={project.metadata.id} />

      {/* Plan approval gate — surfaces pending plan-approval requests for the current
          project. Rendered beside the push card; always visible when pending requests
          exist, regardless of the active dock tab. */}
      <PlanApprovalCard projectId={project.metadata.id} />

      {/* ---- Main: rail + center terminal ---- */}
      <div className="grid grid-cols-1 gap-4 lg:grid-cols-[260px_minmax(0,1fr)]">
        <ProjectWorkspaceAgentRail
          sessions={sessions}
          selectedAgentId={selectedAgentId}
          onSelectAgent={setSelectedAgentId}
          projectId={project.metadata.id}
          projectTitle={project.metadata.title}
          tasks={project.state.tasks}
          projectActive={canLaunch}
          isBusy={isBusy}
          launchMessage={launchMessage}
          rules={rules}
          customClients={customClients}
          localCoderModel={localCoderModel}
          onLaunch={onLaunch}
          onCopyPrompt={onCopyPrompt}
          launcherOpen={launcherOpen}
          onToggleLauncher={() => setLauncherOpen((open) => !open)}
        />

        <section className="min-w-0">
          {selectedSession ? (
            <div className="space-y-3">
              <div className="flex items-center justify-between gap-2">
                <div className="flex min-w-0 items-center gap-2">
                  <span className="truncate text-[12px] font-semibold text-cream-700">
                    {selectedSession.agentId}
                  </span>
                  <TokenUsageBadge usage={tokenUsage} />
                </div>
                <div className="flex shrink-0 items-center gap-2">
                  {/* MC-P7: the Compact action — ONLY for a selected Claude agent
                      (resolved client === "claude"). Runs `/compact` in the agent's
                      terminal to shrink its context window. Independent of the mini
                      Stop brake: a claude coder shows Compact (not Stop); a mini
                      shows Stop (not Compact, unless it is itself a claude mini). */}
                  {selectedCanCompact && (
                    <button
                      type="button"
                      onClick={compactSelected}
                      className="inline-flex items-center gap-1 rounded-2xl border border-teal bg-teal px-2.5 py-0.5 text-[10px] font-semibold text-white hover:bg-teal/90"
                      data-help-title="Runs /compact in this Claude agent to shrink its context."
                      data-help-lines="Sends the /compact slash command to this Claude agent's terminal.|Claude Code summarizes the conversation so far, freeing context window so the agent can keep working longer.|Only Claude agents show this button — /compact is a Claude Code command.|It is a one-click convenience; you can also type /compact yourself in the reply bar."
                    >
                      <Minimize2 className="h-3 w-3" aria-hidden />
                      Compact
                    </button>
                  )}
                  {/* MC-P5: the Stop (kill) safety brake — ONLY for a selected mini.
                      1-click (no two-step confirm): a human Stop is an immediate
                      override. A non-mini agent never shows THIS; it shows the
                      normal-agent Stop button below (the restored stop_agent flow). */}
                  {selectedIsMini && (
                    <button
                      type="button"
                      onClick={stopMini}
                      className="inline-flex items-center gap-1 rounded-2xl border border-coral bg-coral px-2.5 py-0.5 text-[10px] font-semibold text-white hover:bg-coral-dark"
                      data-help-title="Stop this mini-coder now."
                      data-help-lines="Immediately kills this mini-coder; the parent coder will be told it was aborted and must escalate to you.|This is a one-click safety brake — there is no confirm step.|Only mini-coders show this button; a normal agent is stopped from its own controls."
                    >
                      <OctagonX className="h-3 w-3" aria-hidden />
                      Stop
                    </button>
                  )}
                  {/* Open CLI — ONLY for a selected agent that runs in an EXTERNAL
                      console (no in-app PTY). Focuses its dedicated OS terminal
                      window via the parent's focus_agent_terminal. Hidden for an
                      app-hosted agent (its terminal is the in-app viewer). */}
                  {selectedShowsOpenCli && (
                    <button
                      type="button"
                      onClick={focusCliSelected}
                      className="inline-flex items-center gap-1 rounded-md border border-cream-200 bg-white px-2 py-0.5 text-[10px] font-semibold text-cream-600 hover:text-teal"
                      data-help-title="This focuses this agent's external terminal window."
                      data-help-lines="Open CLI restores and brings this agent's dedicated console window to the front.|It applies to externally launched agents (their own OS console).|If the window was closed, you get a friendly error instead.|Relaunch from the Spawn panel if the terminal is gone."
                    >
                      <Terminal className="h-3 w-3" aria-hidden />
                      Open CLI
                    </button>
                  )}
                  {/* Recovery — copy a reconnect prompt for the selected agent.
                      Pure client-side text (no backend call, no secret). Always
                      available for a selected agent; most useful when its heartbeat
                      has gone stale but its terminal may still be alive. */}
                  {selectedSession && (
                    <button
                      type="button"
                      onClick={copyRecoverySelected}
                      className="inline-flex items-center gap-1 rounded-md border border-amber/30 bg-amber/[0.08] px-2 py-0.5 text-[10px] font-semibold text-amber-dark hover:bg-amber/[0.14]"
                      data-help-title="This copies a recovery prompt for this agent."
                      data-help-lines="Recovery is for an agent whose heartbeat went stale but whose terminal may still be open.|It copies exact reconnect steps without exposing hidden tokens.|If the agent lost its session token, relaunch instead.|Always verify the terminal root before trusting status updates."
                    >
                      <LifeBuoy className="h-3 w-3" aria-hidden />
                      Recovery
                    </button>
                  )}
                  {/* Normal-agent Stop — the restored board-mode stop_agent
                      capability, the ONLY UI surface to kill a stalled/runaway
                      NORMAL (non-mini) agent. Mutually exclusive with the mini
                      brake above: a mini shows that brake, a normal agent shows
                      this. Kills the agent's process tree via the parent. */}
                  {selectedShowsNormalStop && (
                    <button
                      type="button"
                      onClick={stopSelected}
                      className="inline-flex items-center gap-1 rounded-md border border-cream-200 bg-white px-2 py-0.5 text-[10px] font-semibold text-cream-600 hover:text-coral-dark"
                      data-help-title="This stops the agent session."
                      data-help-lines="Stop ends the launched agent.|For an app-hosted agent it kills the PTY child; for an external one it closes its console.|It does not delete the task; it only ends the agent.|Relaunch from the Spawn panel if you still need the work done."
                    >
                      <Square className="h-3 w-3" aria-hidden />
                      Stop
                    </button>
                  )}
                  <button
                    type="button"
                    onClick={() => setDrawerOpen((open) => !open)}
                    aria-expanded={drawerOpen}
                    className={`inline-flex items-center gap-1 rounded-md border px-2 py-0.5 text-[10px] font-semibold ${
                      drawerOpen
                        ? "border-terracotta bg-terracotta/10 text-terracotta"
                        : "border-cream-200 bg-white text-cream-600 hover:text-terracotta"
                    }`}
                  >
                    <PanelRightOpen className="h-3 w-3" aria-hidden />
                    drawer ▸
                  </button>
                </div>
              </div>

              {/* Mount the live terminal ONLY for an app-hosted agent (one with a
                  live in-app PTY). Key by agentId so switching agents remounts the
                  viewer cleanly; the lazy xterm chunk loads once and is reused. An
                  external/legacy agent has no app PTY — show a tidy note instead of
                  the viewer's snapshot-error banner. */}
              {ptyAgents.has(selectedSession.agentId) ? (
                <Suspense
                  fallback={
                    <div className="rounded-2xl border border-cream-200 bg-cream-50 px-3 py-10 text-center text-[11px] text-cream-400">
                      Loading terminal…
                    </div>
                  }
                >
                  <AgentTerminalViewer
                    key={selectedSession.agentId}
                    agentId={selectedSession.agentId}
                  />
                </Suspense>
              ) : (
                <div className="flex h-72 items-center justify-center rounded-2xl border border-dashed border-cream-200 bg-cream-50 px-4 text-center text-[12px] text-cream-400">
                  This agent runs in an external console — no in-app terminal to
                  show. Use the drawer for its claims and events.
                </div>
              )}

              {drawerOpen && (
                <AgentDetailDrawer
                  session={selectedSession}
                  claims={claims}
                  events={events}
                  now={now}
                />
              )}

              {/* Question card: shown when the selected agent is waiting for a
                  human reply to a question it raised via ask_user / pendingQuestion. */}
              <AgentQuestionCard session={selectedSession} />
            </div>
          ) : (
            <div className="flex h-72 items-center justify-center rounded-2xl border border-dashed border-cream-200 bg-cream-50 text-center text-[12px] text-cream-400">
              {sessions.length === 0
                ? "No agent working this project. Use + Launch to spawn a coder or verifier."
                : "Select an agent from the left to view its terminal."}
            </div>
          )}
        </section>
      </div>

      {/* ---- Task board (relocated from the board-mode panel), above the dock ---- */}
      {taskBoardSlot}

      {/* ---- Bottom dock ---- */}
      <div className="rounded-2xl border border-cream-200 bg-white">
        <div className="flex w-fit gap-1 border-b border-cream-200 p-1">
          {DOCK_TABS.map((tab) => {
            const active = dockTab === tab.id;
            // The Console tab carries a run pill (spinner + count) while a mini run
            // is active for the selected agent — mirroring the mock's tab badge.
            const showConsoleRun = tab.id === "console" && consoleRunning;
            return (
              <button
                key={tab.id}
                type="button"
                onClick={() => setDockTab(tab.id)}
                className={`inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-[12px] font-semibold transition-colors ${
                  active
                    ? "bg-terracotta text-white"
                    : "text-cream-500 hover:bg-cream-50 hover:text-cream-700"
                }`}
              >
                {tab.label}
                {showConsoleRun ? (
                  <span
                    className={`inline-flex items-center gap-1 rounded-full px-1.5 text-[10px] font-bold ${
                      active
                        ? "bg-white/20 text-white"
                        : "bg-indigo/15 text-indigo-dark"
                    }`}
                  >
                    <span
                      className={`h-2 w-2 animate-spin rounded-full border-[1.6px] border-transparent motion-reduce:animate-none ${
                        active
                          ? "border-t-white"
                          : "border-indigo-light border-t-indigo"
                      }`}
                      aria-hidden
                    />
                    {consoleCount}
                  </span>
                ) : null}
              </button>
            );
          })}
        </div>

        <div className="p-4">
          {dockTab === "censor" && (
            <CensorPanel
              projectId={project.metadata.id}
              root={project.metadata.rootPath}
              onLaunch={onLaunch}
              isBusy={isBusy}
              canLaunch={canLaunch}
            />
          )}

          {dockTab === "git" && <DockGit project={project} />}

          {dockTab === "plans" && (
            <PlansDockTab projectId={project.metadata.id} />
          )}

          {dockTab === "console" && (
            // CONCRETE height (the mock's dock-body height) so BOTH the empty
            // state's `flex-1 h-full justify-center` can actually center AND a long
            // timeline scrolls inside the panel. A max-h/min-h pair leaves the
            // flex child without a definite height, so the empty state pins to the
            // top instead of centering.
            <div className="flex h-[348px] flex-col">
              <AgentTimelineStrip activity={consoleActivity} />
              <div className="min-h-0 flex-1 overflow-y-auto">
                <AgentConsole activity={consoleActivity} />
              </div>
              {(consoleActivity?.taskCostEstimateUsd != null || costTotalUsd != null) && (
                <div className="flex items-center justify-end gap-3 border-t border-cream-100 px-3 py-1 text-[11px] text-cream-500">
                  {consoleActivity?.taskCostEstimateUsd != null && (
                    <span>
                      est. task ~${consoleActivity.taskCostEstimateUsd.toFixed(4)}
                    </span>
                  )}
                  {costTotalUsd != null && (
                    <span>total ~${costTotalUsd.toFixed(2)} (est)</span>
                  )}
                </div>
              )}
              <MiniSteerBar agentId={selectedAgentId} />
            </div>
          )}

          {dockTab === "mcp" &&
            (project.metadata.rootPath ? (
              <ProjectMcpServersCard projectRoot={project.metadata.rootPath} />
            ) : (
              <p className="text-[11px] text-cream-400">
                No project root path — cannot load MCP servers.
              </p>
            ))}

          {dockTab === "changes" && <ChangesDockTab project={project} />}
        </div>
      </div>

      {/* ---- Notes (relocated from the board-mode panel), below the dock ---- */}
      {notesSlot}
    </div>
  );
}

// ---- Git tab: the gitStatus detail (branch, ahead/behind, counts, upstream) --

function DockGit({ project }: { project: ProjectDetail }) {
  const git = project.gitStatus;
  // Defensive: gitStatus is typed non-null but the backend can omit it (other call
  // sites — projectWorkspaceModel/censorCounts — treat it as nullable), so guard so
  // the Git dock tab renders a message instead of crashing on a missing status.
  if (!git || !git.isGitRepo) {
    return (
      <p className="text-[11px] text-cream-400">
        The project root is not inside a Git repository.
      </p>
    );
  }
  const rows: { label: string; value: string }[] = [
    { label: "Branch", value: git.branch ?? "—" },
    { label: "Upstream", value: git.upstream ?? "no upstream" },
    { label: "Ahead / Behind", value: `↑${git.aheadCount} / ↓${git.behindCount}` },
    { label: "Staged", value: String(git.stagedCount) },
    { label: "Unstaged", value: String(git.unstagedCount) },
    { label: "Untracked", value: String(git.untrackedCount) },
    { label: "Dirty total", value: String(git.dirtyCount) },
    { label: "Last commit", value: git.commit ?? "—" },
  ];
  return (
    <dl className="grid grid-cols-1 gap-x-6 gap-y-2 sm:grid-cols-2">
      {rows.map((row) => (
        <div key={row.label} className="flex items-center justify-between gap-3">
          <dt className="text-[10px] font-semibold uppercase tracking-widest text-cream-500">
            {row.label}
          </dt>
          <dd className="truncate font-mono text-[11px] text-cream-700">
            {row.value}
          </dd>
        </div>
      ))}
    </dl>
  );
}

export default ProjectWorkspace;
