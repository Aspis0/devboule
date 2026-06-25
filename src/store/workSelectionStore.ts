// Bridges the Work Console (LivingPlan/FocusStage) and the bottom DAG board (TaskCard).
// The agent<->task mapping lives in the helpers (not the store) because it needs live session/claim data.
//
// Twinning contract (enforced at the CALL SITES, not structurally here): a consumer that selects
// one side must set BOTH ids in tandem so the two surfaces never half-desync — selecting an agent
// resolves its task via `taskIdForAgent`; selecting a task resolves its agent via
// `primaryAgentForTask`; deselecting uses `clear()`. To keep the board and console in agreement,
// `primaryAgentForTask` is fed the SAME per-card claim/session arrays the board uses for its
// badges, so the resolved "primary" agent is exactly the first badge the user sees on that card.
import { create } from "zustand";
import type { AgentSession, AgentClaim } from "../types/backend";
import { deriveWorkers } from "../components/projects/agentBadge";

interface WorkSelectionState {
  selectedAgentId: string | null;
  selectedTaskId: string | null;
  // The ONLY mutators are paired: `selectBoth` sets the agent AND its task in one store write
  // (subscribers never observe a half-updated snapshot), `clear` resets both. There is NO
  // single-field setter on purpose — setting one id without the other would break the twinning
  // invariant, so it is not exposed.
  selectBoth: (agentId: string | null, taskId: string | null) => void;
  clear: () => void;
}

export const useWorkSelectionStore = create<WorkSelectionState>((set) => ({
  selectedAgentId: null,
  selectedTaskId: null,
  selectBoth: (agentId, taskId) =>
    set({ selectedAgentId: agentId, selectedTaskId: taskId }),
  clear: () => set({ selectedAgentId: null, selectedTaskId: null }),
}));

export function taskIdForAgent(
  agentId: string | null,
  sessions: AgentSession[],
  claims: AgentClaim[]
): string | null {
  // Falsy guard also rejects "" / undefined a later caller might pass from an unset field.
  if (!agentId) return null;
  const session = sessions.find((s) => s.agentId === agentId);
  if (session?.currentTaskId) {
    return session.currentTaskId;
  }
  const claim = claims.find((c) => c.agentId === agentId);
  return claim?.taskId ?? null;
}

export function primaryAgentForTask(
  taskId: string | null,
  claims: AgentClaim[],
  sessions: AgentSession[]
): string | null {
  if (!taskId) return null;
  const filteredClaims = claims.filter((c) => c.taskId === taskId);
  const filteredSessions = sessions.filter((s) => s.currentTaskId === taskId);
  const workers = deriveWorkers(filteredClaims, filteredSessions);
  return workers[0]?.agentId ?? null;
}
