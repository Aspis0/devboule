// Phase C (split view): a SELF-CONTAINED focus pane for ONE agent. It owns the per-agent
// hooks (useAgentConsole), the Activity/Raw view state, the two-way dispatch (composer +
// quick actions + answer), and the raw PTY terminal slot. Extracting this lets ProjectWorkspace
// render it once (single view) OR twice side by side (split), each pane a distinct component
// instance so the per-agent hooks are ALWAYS called unconditionally — no conditional-hook
// violation when the second pane appears/disappears.

import { Suspense, lazy, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { X } from "lucide-react";
import { invokeBackendCommand } from "../../context/AppContext";
import { useAgentConsole } from "../agents/useAgentConsole";
import { FocusStage } from "./FocusStage";
import {
  findWorkNode,
  type WorkConsoleModel,
  type WorkNode,
} from "./workConsoleModel";
import { agentChannel, type CommsDirection } from "./agentChannel";
import { stripSpoofChars } from "../agents/attentionNotifier";
import { isMiniManagedSession } from "../projects/projectWorkspaceModel";
import type { AgentSession } from "../../types/backend";

const AgentTerminalViewer = lazy(() =>
  import("../agents/AgentTerminalViewer").then((m) => ({
    default: m.AgentTerminalViewer,
  })),
);

// Canned steer messages for the FocusStage quick-action chips (Direction A).
const FOCUS_QUICK_ACTIONS: Record<"redo" | "narrow" | "pause", string> = {
  redo: "Redo this round.",
  narrow: "Narrow the scope to the current file only.",
  pause: "Pause after the current step.",
};

export interface FocusStagePaneProps {
  agentId: string;
  model: WorkConsoleModel;
  sessions: AgentSession[];
  ptyAgents: Set<string>;
  readOnly?: boolean;
  // When set, the pane shows a close (✕) affordance — used for the pinned SECOND pane in
  // split view so the user can collapse back to a single focus.
  onClose?: () => void;
}

export function FocusStagePane({
  agentId,
  model,
  sessions,
  ptyAgents,
  readOnly,
  onClose,
}: FocusStagePaneProps) {
  const node = useMemo(() => findWorkNode(model, agentId), [model, agentId]);
  const session = useMemo(
    () => sessions.find((s) => s.agentId === agentId) ?? null,
    [sessions, agentId],
  );
  const activity = useAgentConsole(agentId);

  const [view, setView] = useState<"activity" | "raw">("activity");
  // Transient error for a failed dispatch (send/quick-action/answer) so a dropped
  // message isn't silent — FocusStage has no error slot, so we keep it local here.
  const [dispatchError, setDispatchError] = useState<string | null>(null);
  const dispatchErrorTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Reset to Activity whenever this pane's agent changes, so a Raw view (or its external-
  // console note) never leaks across agents. Also clear any transient dispatch error
  // and its pending auto-dismiss timer (the cleanup runs on agent-switch AND unmount).
  useEffect(() => {
    setView("activity");
    setDispatchError(null);
    return () => {
      if (dispatchErrorTimerRef.current) {
        clearTimeout(dispatchErrorTimerRef.current);
        dispatchErrorTimerRef.current = null;
      }
    };
  }, [agentId]);

  // Direction A/B dispatch through a ref so the callbacks stay stable across the 5s sessions
  // poll (no FocusStage timeline re-render every tick). Null the node when there is no resolved
  // session, so a transient selected-but-not-loaded state can't route a message to the wrong
  // channel and silently drop it.
  const dispatchRef = useRef<{ node: WorkNode | null; miniManaged: boolean }>({
    node: null,
    miniManaged: true,
  });
  // Refresh the dispatch target in a COMMIT-phase effect (not the render body), so an
  // aborted/concurrent render can never leave the ref pointing at the wrong agent. Runs every
  // commit (no deps) — cheap, and a user send only happens after paint, so the ref is current.
  useEffect(() => {
    dispatchRef.current = {
      node: session ? node : null,
      miniManaged: session ? isMiniManagedSession(session) : false,
    };
  });
  const dispatch = useCallback((text: string, dir: CommsDirection) => {
    const { node: target, miniManaged } = dispatchRef.current;
    const t = text.trim();
    if (!t || !target) return;
    const ch = agentChannel(target, { miniManaged }, dir);
    if (!ch) return;
    void invokeBackendCommand(ch.command, ch.buildArgs(t)).catch((e) => {
      // A failed backend command must surface instead of silently dropping the message.
      const message =
        e instanceof Error ? e.message : "The message could not be sent.";
      setDispatchError(message);
      if (dispatchErrorTimerRef.current) clearTimeout(dispatchErrorTimerRef.current);
      dispatchErrorTimerRef.current = setTimeout(() => setDispatchError(null), 5000);
    });
  }, [setDispatchError]);
  const onSendMessage = useCallback((t: string) => dispatch(t, "message"), [dispatch]);
  const onAnswer = useCallback((t: string) => dispatch(t, "answer"), [dispatch]);
  const onQuickAction = useCallback(
    (a: "redo" | "narrow" | "pause") => dispatch(FOCUS_QUICK_ACTIONS[a], "message"),
    [dispatch],
  );
  // The worker composer can only message a coder/mini. The orchestrator (planner console) and
  // the censor (automated) have no channel here — disable the composer rather than drop sends.
  const composerDisabled =
    !!readOnly || (node != null && node.type !== "coder" && node.type !== "mini");

  if (!node) {
    return (
      <div className="relative flex h-full items-center justify-center rounded-2xl border border-dashed border-cream-200 bg-cream-50 px-4 text-center text-[12px] text-cream-400">
        {onClose ? <CloseButton onClose={onClose} /> : null}
        This agent isn&apos;t placed in the work model (it may belong to a different project or
        have no current task). Use the drawer for its claims and events.
      </div>
    );
  }

  return (
    <div className="relative h-full overflow-hidden rounded-2xl border border-cream-200 bg-white">
      {onClose ? <CloseButton onClose={onClose} /> : null}
      {dispatchError ? (
        <div className="absolute inset-x-0 top-0 z-20 rounded-t-2xl bg-coral/10 px-3 py-1.5 text-[11px] leading-4 text-coral-dark">
          {dispatchError}
        </div>
      ) : null}
      <FocusStage
        node={node}
        activity={activity}
        view={view}
        onViewChange={setView}
        onSendMessage={onSendMessage}
        pendingQuestion={
          readOnly || !node.pendingQuestion ? null : stripSpoofChars(node.pendingQuestion)
        }
        onAnswer={onAnswer}
        disabled={composerDisabled}
        onQuickAction={onQuickAction}
        rawSlot={
          session && ptyAgents.has(session.agentId) ? (
            <Suspense
              fallback={
                <div className="rounded-2xl border border-cream-200 bg-cream-50 px-3 py-10 text-center text-[11px] text-cream-400">
                  Loading terminal…
                </div>
              }
            >
              <AgentTerminalViewer key={session.agentId} agentId={session.agentId} />
            </Suspense>
          ) : (
            <div className="flex h-full items-center justify-center px-4 text-center text-[12px] text-cream-400">
              This agent runs in an external console — no in-app terminal to show. Use the
              drawer for its claims and events.
            </div>
          )
        }
      />
    </div>
  );
}

function CloseButton({ onClose }: { onClose: () => void }) {
  return (
    <button
      type="button"
      onClick={onClose}
      aria-label="Close this split pane"
      title="Close split"
      className="absolute right-2 top-2 z-10 inline-flex h-6 w-6 items-center justify-center rounded-md border border-cream-200 bg-white/90 text-cream-500 hover:text-terracotta"
    >
      <X className="h-3.5 w-3.5" aria-hidden />
    </button>
  );
}
