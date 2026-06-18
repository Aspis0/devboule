// Zustand store holding the LATEST agent sessions for the "needs you" surfaces
// (Header bell pill + OS notifications). Mirrors cityStore's style.
//
// This store does NOT fetch anything itself — it is a passive sink fed by exactly
// ONE active feeder at any moment (the "single feeder" invariant):
//   - When the Projects view is active: ProjectsView's existing agent-live-state
//     poll (loadAgentState, ~5s) pushes here via setFromLiveState.
//   - Everywhere else (the Projects Board/Polis/Oracle/Cloudflare/...): the GLOBAL
//     attention poller in App.tsx (AppShell) takes over, on the same ~5s cadence,
//     visibility-gated + in-flight-guarded. It SKIPS its tick while activeView ===
//     "projects" so the two never both fetch get_agent_live_state at once.
// After Phase G (the standalone Agents page was dissolved) the bell is the PRIMARY
// needs-you signal, so this coverage must be global — hence the App-level poller.
// The Header reads the store reactively; the app-level attention watcher subscribes
// to it for OS notifications.
//
// We keep the WHOLE session list (not a pre-filtered attention subset) so the
// single attention predicate (attentionSessions in agentFleet.ts) stays the only
// place that decides what needs the human — readers reuse it, never re-implement.

import { create } from "zustand";
import type { AgentLiveState, AgentSession } from "../types/backend";

interface AgentAttentionState {
  /** Latest sessions from the most recent live-state fetch (any feeder). */
  sessions: AgentSession[];
  /** Bumped to Date.now() on every accepted update, so subscribers can tell two
   *  structurally-equal snapshots apart and the Header can show freshness. */
  updatedAt: number;
  /** Replace the stored sessions from a fetched live state. A null state (no
   *  data / failed fetch) clears to an empty fleet. SKIPS set() when the snapshot
   *  is structurally unchanged (same updatedAt + sizes + needsUser-since join) so
   *  a 5s poll does not hand subscribers a new array reference every tick (which
   *  re-rendered the Header). updatedAt only advances when content actually moved,
   *  so the watcher still re-evaluates real needsUser transitions. */
  setFromLiveState: (state: AgentLiveState | null) => void;
}

/** Cheap structural signature of a sessions snapshot for the setFromLiveState
 *  skip. Combines length with a per-session id + needsUser.since join so a
 *  needsUser transition (enter/leave/re-raise) always changes the signature even
 *  when the session count is unchanged, while a pure heartbeat tick (no relevant
 *  change) keeps it stable. updatedAt is deliberately EXCLUDED: it bumps every
 *  poll on the backend, so including it would defeat the skip. */
function sessionsSignature(sessions: AgentSession[]): string {
  let sig = `${sessions.length}`;
  for (const s of sessions) {
    sig += `|${s.agentId}:${s.needsUser?.since ?? ""}`;
  }
  return sig;
}

export const useAgentAttentionStore = create<AgentAttentionState>(
  (set, get) => ({
    sessions: [],
    updatedAt: 0,
    setFromLiveState: (state) => {
      const next = state?.sessions ?? [];
      const prev = get().sessions;
      if (sessionsSignature(prev) === sessionsSignature(next)) {
        // Structurally identical to the stored snapshot: keep the SAME array
        // reference (and updatedAt) so Header's `sessions` selector does not
        // re-render and the watcher does not re-fire on a no-op poll.
        return;
      }
      set({ sessions: next, updatedAt: Date.now() });
    },
  }),
);

/** The store API type (getState/subscribe/setState) for non-React consumers like
 *  the app-level attention watcher. */
export type AgentAttentionStore = typeof useAgentAttentionStore;
