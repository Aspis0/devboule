// THE canonical agent row, shared by the global Agents fleet view and the
// per-project ProjectAgentPanel. It renders one live agent: status dot + health,
// id, model badge, role, CLI badge, project/task line, heartbeat age, a subagent
// chip, a needs-you badge, and the row actions. An expandable detail drawer
// (AgentDetailDrawer) and the lazy in-app terminal mount UNDER the row.
//
// All display derivations come from the pure agentRowModel helpers; this file is
// the thin JSX shell. Health vocab/tones come from ../projects/agentLiveStatus so
// the two surfaces never drift. Every interactive element keeps the data-help-*
// idiom used across the app.

import {
  AlertTriangle,
  Castle,
  ChevronDown,
  ChevronRight,
  Copy,
  LifeBuoy,
  Square,
  SquareTerminal,
  Terminal,
  Users,
} from "lucide-react";
import { useState, type ReactNode } from "react";
import type {
  AgentClaim,
  AgentEvent,
  AgentSession,
} from "../../types/backend";
import {
  healthTone,
  healthWord,
  type LiveStatusTone,
} from "../projects/agentLiveStatus";
import { rowBadges, rowActions } from "./agentRowModel";
import { AgentDetailDrawer } from "./AgentDetailDrawer";

// Live-dot tone -> dot color + word color, mirroring ProjectAgentPanel.
const liveDotClass: Record<LiveStatusTone, string> = {
  working: "bg-sage-dark",
  idle: "bg-amber-dark",
  stalled: "bg-coral-dark",
};
const liveWordClass: Record<LiveStatusTone, string> = {
  working: "text-sage-dark",
  idle: "text-amber-dark",
  stalled: "text-coral-dark",
};
const heartbeatClass: Record<LiveStatusTone, string> = {
  working: "text-cream-400",
  idle: "text-amber-dark",
  stalled: "text-coral-dark",
};

const roleTone: Record<string, string> = {
  orchestrator: "bg-terracotta/10 text-terracotta",
  coder: "bg-teal/10 text-teal",
  verifier: "bg-sage/10 text-sage-dark",
};

export interface AgentRowProps {
  session: AgentSession;
  // Shared live clock so age/health recompute between polls.
  now: number;
  // Human task label ("T-1 / Title") when the parent can resolve it.
  taskLabel?: string | null;
  // Claims + events arrays the drawer filters down to this agent.
  claims: AgentClaim[];
  events: AgentEvent[];
  // Whether this agent currently has a live app-hosted PTY (from agent_pty_list).
  // The Terminal toggle only renders when true.
  hasAppTerminal: boolean;
  // Terminal viewer open state + toggle. When open, `renderTerminal` is mounted
  // under the row by the parent (keeps the lazy xterm import out of this file).
  terminalOpen: boolean;
  onToggleTerminal?: (agentId: string) => void;
  renderTerminal?: () => ReactNode;
  // Actions (signatures match the existing ProjectsView/AgentsView wiring).
  onViewInPolis?: (session: AgentSession) => void;
  onOpenCli?: (agentId: string) => void; // external console focus
  onStop?: (agentId: string) => void;
  onRecovery?: (session: AgentSession) => void;
  recoveryCopied?: boolean;
  // Ref callback so the parent can scroll a deep-linked row into view.
  rowRef?: (el: HTMLElement | null) => void;
}

export function AgentRow({
  session,
  now,
  taskLabel,
  claims,
  events,
  hasAppTerminal,
  terminalOpen,
  onToggleTerminal,
  renderTerminal,
  onViewInPolis,
  onOpenCli,
  onStop,
  onRecovery,
  recoveryCopied = false,
  rowRef,
}: AgentRowProps) {
  const badges = rowBadges(session, now);
  const tone = healthTone(badges.health);
  const word = healthWord(badges.health);
  // Action gating from the session's terminal host + live-PTY state. Terminal
  // toggle only for a live in-app PTY; Open CLI only for a non-app host; an
  // app-hosted agent whose PTY exited gets a relaunch hint instead of dead
  // buttons. See agentRowModel.rowActions for the rules.
  const actions = rowActions(session, hasAppTerminal);

  const [confirmingStop, setConfirmingStop] = useState(false);
  const [expanded, setExpanded] = useState(false);

  return (
    <article
      ref={(el) => rowRef?.(el)}
      className={`rounded-lg border p-3 ${
        session.needsUser
          ? "border-amber/40 bg-amber/[0.06]"
          : badges.health === "lost"
            ? "border-coral/20 bg-coral/[0.03]"
            : badges.health === "stale" || badges.health === "unknown"
              ? "border-amber/20 bg-amber/[0.04]"
              : "border-cream-200 bg-cream-50"
      }`}
      data-help-title={`${session.agentId} is a live ${badges.cli.label} agent.`}
      data-help-lines="Each row is one agent: the dot/word is live status from heartbeat age, the badges show model, role, and CLI, and the right side is when it last checked in.|A subagent chip means this agent reported a fan-out of helpers (advisory, self-reported over MCP).|An amber needs-you badge means the agent is blocked waiting on you; open its terminal to answer.|Read this before launching another agent so work is not duplicated."
    >
      {/* Line 1: status, id, model, role, CLI + heartbeat (right). */}
      <div className="flex items-center gap-2">
        <span
          className={`h-2 w-2 shrink-0 rounded-full ${liveDotClass[tone]}`}
          aria-hidden
        />
        <span
          className={`text-[11px] font-semibold uppercase tracking-wide ${liveWordClass[tone]}`}
        >
          {word}
        </span>
        <span className="truncate text-[12px] font-semibold text-cream-800">
          {session.agentId}
        </span>
        <span
          className={`shrink-0 rounded-md px-1.5 py-0.5 text-[10px] font-semibold ${
            badges.modelKnown
              ? "bg-white text-cream-600"
              : "bg-cream-100 text-cream-400"
          }`}
          title={badges.modelLabel}
        >
          {badges.modelLabel}
        </span>
        <span
          className={`shrink-0 rounded-md px-1.5 py-0.5 text-[10px] font-semibold capitalize ${
            roleTone[session.role.toLowerCase()] ?? "bg-white text-cream-500"
          }`}
        >
          {session.role}
        </span>
        <span
          className={`shrink-0 rounded-md px-1.5 py-0.5 text-[10px] font-semibold ${badges.cli.toneClass}`}
        >
          {badges.cli.label}
        </span>
        <span
          className={`ml-auto flex shrink-0 items-center gap-1 text-[10px] font-semibold ${heartbeatClass[tone]}`}
          title="Heartbeat age"
        >
          {tone !== "working" && (
            <AlertTriangle className="h-3 w-3" aria-hidden />
          )}
          <span aria-hidden>&hearts;</span>
          {badges.ageLabel}
        </span>
      </div>

      {/* Line 2: project/task + last message. */}
      <p className="mt-1 truncate text-[10px] text-cream-500">
        {session.currentProjectId ? `${session.currentProjectId} · ` : ""}
        {taskLabel ?? session.currentTaskId ?? "project-level"}
        {session.message ? ` · ${session.message}` : ""}
      </p>

      {/* Subagent chips + needs-you badge. */}
      {(badges.subagentChips.length > 0 || badges.needsUserMessage) && (
        <div className="mt-1.5 flex flex-wrap items-center gap-1.5">
          {badges.subagentChips.map((chip, i) => (
            <span
              key={`${i}-${chip}`}
              className="inline-flex items-center gap-1 rounded-md bg-white px-1.5 py-0.5 text-[10px] font-semibold text-cream-600"
            >
              <Users className="h-3 w-3" aria-hidden />
              {chip}
            </span>
          ))}
          {badges.needsUserMessage && (
            <span
              className="inline-flex min-w-0 items-center gap-1 rounded-md bg-amber/20 px-1.5 py-0.5 text-[10px] font-semibold text-amber-dark"
              title={badges.needsUserMessage}
            >
              <AlertTriangle className="h-3 w-3 shrink-0" aria-hidden />
              <span className="truncate">Needs you: {badges.needsUserMessage}</span>
            </span>
          )}
        </div>
      )}

      {/* Actions. */}
      <div className="mt-1.5 flex flex-wrap items-center gap-1.5">
        {actions.showTerminalToggle && onToggleTerminal && (
          <button
            type="button"
            onClick={() => onToggleTerminal(session.agentId)}
            data-help-title="This opens the agent's in-app terminal."
            data-help-lines="The in-app terminal mirrors an app-hosted agent's live output.|You can type directly into the grid (e.g. /compact, /quit) or use the reply bar.|Only agents launched inside the app (host=app) show this control.|Closing the panel detaches the viewer but does not stop the agent."
            className={`inline-flex items-center gap-1 rounded-md border px-2 py-0.5 text-[10px] font-semibold ${
              terminalOpen
                ? "border-terracotta bg-terracotta/10 text-terracotta"
                : "border-cream-200 bg-white text-cream-600 hover:text-terracotta"
            }`}
          >
            <SquareTerminal className="h-3 w-3" aria-hidden />
            {terminalOpen ? "Hide terminal" : "View terminal"}
          </button>
        )}
        {onViewInPolis && (
          <button
            type="button"
            onClick={() => onViewInPolis(session)}
            data-help-title="This opens Polis focused on this agent."
            data-help-lines="Polis is the live city map of the codebase.|This switches to Polis, maps the agent's project folder, and selects the agent once the city loads.|If the agent's folder is not under the mapped tree it stays in the off-map roster.|The agent's position follows the live agent state."
            className="inline-flex items-center gap-1 rounded-md border border-cream-200 bg-white px-2 py-0.5 text-[10px] font-semibold text-cream-600 hover:text-terracotta"
          >
            <Castle className="h-3 w-3" aria-hidden />
            View in Polis
          </button>
        )}
        {actions.showOpenCli && onOpenCli && (
          <button
            type="button"
            onClick={() => onOpenCli(session.agentId)}
            data-help-title="This focuses this agent's external terminal window."
            data-help-lines="Open CLI restores and brings this agent's dedicated console window to the front.|It applies to externally launched agents (their own OS console).|If the window was closed, you get a friendly error instead.|Relaunch from the Spawn panel if the terminal is gone."
            className="inline-flex items-center gap-1 rounded-md border border-cream-200 bg-white px-2 py-0.5 text-[10px] font-semibold text-cream-600 hover:text-teal"
          >
            <Terminal className="h-3 w-3" aria-hidden />
            Open CLI
          </button>
        )}
        {actions.showExitedHint && (
          <span
            className="inline-flex items-center gap-1 rounded-md border border-cream-200 bg-cream-100 px-2 py-0.5 text-[10px] font-semibold text-cream-400"
            title="This agent ran in an in-app terminal that has exited. Relaunch it from the Spawn panel to reattach a terminal."
          >
            <SquareTerminal className="h-3 w-3" aria-hidden />
            Terminal exited — relaunch to reattach
          </span>
        )}
        {badges.recovery && onRecovery && (
          <button
            type="button"
            onClick={() => onRecovery(session)}
            data-help-title="This copies a recovery prompt for this stalled agent."
            data-help-lines="Recovery is for an agent whose heartbeat went stale but whose terminal may still be open.|It copies exact reconnect steps without exposing hidden tokens.|If the agent lost its session token, relaunch instead.|Always verify the terminal root before trusting status updates."
            className="inline-flex items-center gap-1 rounded-md border border-amber/30 bg-amber/[0.08] px-2 py-0.5 text-[10px] font-semibold text-amber-dark hover:bg-amber/[0.14]"
          >
            {recoveryCopied ? (
              <Copy className="h-3 w-3" aria-hidden />
            ) : (
              <LifeBuoy className="h-3 w-3" aria-hidden />
            )}
            {recoveryCopied ? "Copied" : "Recovery"}
          </button>
        )}
        {onStop &&
          (confirmingStop ? (
            <span className="inline-flex items-center gap-1.5 rounded-md border border-coral/30 bg-coral/[0.06] px-2 py-0.5">
              <span className="text-[10px] font-semibold text-coral-dark">
                Stop {session.agentId}?
              </span>
              <button
                type="button"
                onClick={() => {
                  setConfirmingStop(false);
                  onStop(session.agentId);
                }}
                className="rounded-md bg-coral px-1.5 py-0.5 text-[10px] font-semibold text-white hover:bg-coral/90"
              >
                Confirm
              </button>
              <button
                type="button"
                onClick={() => setConfirmingStop(false)}
                className="rounded-md border border-cream-200 bg-white px-1.5 py-0.5 text-[10px] font-semibold text-cream-600 hover:text-cream-800"
              >
                Cancel
              </button>
            </span>
          ) : (
            <button
              type="button"
              onClick={() => setConfirmingStop(true)}
              data-help-title="This stops the agent session."
              data-help-lines="Stop ends the launched agent.|For an app-hosted agent it kills the PTY child; for an external one it closes its console.|It does not delete the task; it only ends the agent.|Relaunch from the Spawn panel if you still need the work done."
              className="inline-flex items-center gap-1 rounded-md border border-cream-200 bg-white px-2 py-0.5 text-[10px] font-semibold text-cream-600 hover:text-coral-dark"
            >
              <Square className="h-3 w-3" aria-hidden />
              Stop
            </button>
          ))}
        {/* Detail drawer toggle. */}
        <button
          type="button"
          onClick={() => setExpanded((open) => !open)}
          aria-expanded={expanded}
          className="ml-auto inline-flex items-center gap-1 rounded-md border border-cream-200 bg-white px-2 py-0.5 text-[10px] font-semibold text-cream-500 hover:text-cream-700"
        >
          {expanded ? (
            <ChevronDown className="h-3 w-3" aria-hidden />
          ) : (
            <ChevronRight className="h-3 w-3" aria-hidden />
          )}
          Details
        </button>
      </div>

      {/* In-app terminal viewer (lazy, parent-supplied). */}
      {terminalOpen && hasAppTerminal && renderTerminal?.()}

      {/* Detail drawer: this agent's claims, events, subagents. */}
      {expanded && (
        <AgentDetailDrawer
          session={session}
          claims={claims}
          events={events}
          now={now}
        />
      )}
    </article>
  );
}

export default AgentRow;
