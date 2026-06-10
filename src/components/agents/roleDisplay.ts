// Phase B role merge: pure, DOM-free helpers for the spawn-time role set and the
// DERIVED "orchestrator" badge.
//
// Spawn-time roles collapse to {coder, verifier}. "orchestrator" is no longer a
// spawnable role — it survives only as:
//   1. a back-compat INBOUND value on stored sessions / old `.aspis-agents.json`
//      (the AgentRole union still accepts it so old data deserializes), and
//   2. a DERIVED badge shown when a session is currently coordinating subagents
//      OR was stored with the legacy role string.
//
// This mirrors the Rust side (src-tauri/src/polis/scanner.rs `derived_agent_type`)
// and the Python alias (oracle/server/aspis_mcp.py ROLE_ALIASES) so the three
// surfaces never drift.

import type { AgentSession, SpawnRole } from "../../types/backend";

// The only roles a NEW agent can be spawned with after the merge. Re-exported from
// the canonical types module so the many UI importers of `./roleDisplay` keep a
// single source of truth (also used by AgentRoleRule.role in types/backend.ts).
export type { SpawnRole };

export interface DisplayRole {
  // The canonical role to render. A stored legacy "orchestrator" maps to coder.
  role: SpawnRole;
  // Whether to show the derived "Orchestrator" badge alongside the role.
  orchestratorBadge: boolean;
}

// Minimal shape `displayRole` reads: just the stored role and (optional) subagent
// list. Accepts a full AgentSession or any subset with these fields so callers
// (rows, drawers, Polis) can pass whatever they hold.
export type RoleDisplaySource = Pick<AgentSession, "role"> &
  Partial<Pick<AgentSession, "subagents">>;

// Derive how an agent's role should be displayed.
//
//   role             = "verifier" when the stored role is verifier, else "coder"
//                       (this folds the legacy "orchestrator" and any unknown
//                       stored role to coder, matching normalize_agent_role).
//   orchestratorBadge = stored role === "orchestrator" OR (the DISPLAYED role is
//                       NOT verifier AND it currently has >=1 subagent). A verifier
//                       is NEVER promoted, even with subagents — this mirrors the
//                       Rust Polis `derived_agent_type` (scanner.rs), which only
//                       promotes a coder that fanned out. Previously a verifier with
//                       subagents wrongly showed the orchestrator badge.
//
// Null-safe: an absent/empty `subagents` never raises the badge on its own.
export function displayRole(session: RoleDisplaySource): DisplayRole {
  const stored = (session.role ?? "").trim().toLowerCase();
  const subagentCount = session.subagents?.length ?? 0;
  const role: SpawnRole = stored === "verifier" ? "verifier" : "coder";
  const orchestratorBadge =
    stored === "orchestrator" || (role !== "verifier" && subagentCount > 0);
  return { role, orchestratorBadge };
}
