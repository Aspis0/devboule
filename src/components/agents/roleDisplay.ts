// ROLE UNTANGLE (2026-07): pure, DOM-free helpers for the agent-role display.
//
// FOUR first-class roles exist — orchestrator / coder ("Main coder") / verifier /
// mini — and the ledger stores them TRUTHFULLY (a planner launch persists
// role:"orchestrator" whether it runs the local Devboule binary or a cloud CLI in
// duplex mode). So this module is a PASS-THROUGH of the stored role, not a fold:
// the former "derive an orchestrator badge from the subagent count" heuristic is
// dead. Mirrors the Rust single fold (src-tauri/src/backend/agent_role.rs) and the
// Python VALID_ROLES/ROLE_ALIASES (oracle/server/aspis_mcp.py); the Rust Polis
// scanner (`derived_agent_type`) is a pass-through of the same set.

import type { AgentSession, SpawnRole } from "../../types/backend";

// The roles a NEW agent can be spawned with from the SpawnPanel role radio.
// (An orchestrator is launched via the planner / the Devboule client selection,
// not via the radio.) Re-exported so the many UI importers of `./roleDisplay`
// keep a single source of truth.
export type { SpawnRole };

// The role a session row renders. Superset of SpawnRole: a stored orchestrator
// session displays as itself.
export type DisplayableRole = SpawnRole | "orchestrator";

export interface DisplayRole {
  // The stored role, canonicalized: orchestrator/verifier pass through, legacy
  // aliases and unknown strings fold to coder (matching agent_role.rs).
  role: DisplayableRole;
  // True exactly when the stored role IS "orchestrator" — a pure reflection of
  // the ledger, no derivation.
  orchestratorBadge: boolean;
}

// Minimal shape `displayRole` reads: just the stored role and (optional) subagent
// list. Accepts a full AgentSession or any subset with these fields so callers
// (rows, drawers, Polis) can pass whatever they hold. `subagents` is no longer
// consulted (kept in the type so existing callers compile unchanged).
export type RoleDisplaySource = Pick<AgentSession, "role"> &
  Partial<Pick<AgentSession, "subagents">>;

export function displayRole(session: RoleDisplaySource): DisplayRole {
  const stored = (session.role ?? "").trim().toLowerCase();
  const role: DisplayableRole =
    stored === "verifier"
      ? "verifier"
      : stored === "orchestrator"
        ? "orchestrator"
        : "coder";
  return { role, orchestratorBadge: role === "orchestrator" };
}

/** Presentational chip label for the Activity stream (and similar surfaces).
 *  Maps the session's stored role to a human title; unknown/empty → "Agent". */
export function roleChipLabel(role: string | null | undefined): string {
  const stored = (role ?? "").trim().toLowerCase();
  if (stored === "orchestrator") return "Orchestrator";
  if (stored === "verifier") return "Verifier";
  if (stored === "mini") return "Mini";
  if (stored === "coder") return "Coder";
  return "Agent";
}
