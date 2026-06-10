// Pure fleet-aggregation selectors for the Agents control room.
//
// No React, no I/O: given the live AgentSession[] from the backend, fold parent
// sessions and their reported subagents into per-(role, model) counts, render a
// human summary string, and pick the sessions that need human attention.
//
// Mirrors the MCP contract shaped in oracle/server/aspis_mcp.py (subagents +
// needsUser) and the Rust models in src-tauri/src/backend/model.rs.

import type { AgentSession } from "../../types/backend";
import { sessionHealth } from "../projects/agentLiveStatus";

export interface FleetCount {
  role: string;
  model: string;
  count: number;
}

const MODEL_UNKNOWN = "unknown";

// "Closed" notion: a session that is no longer live and must not be counted in
// the fleet. We reuse agentLiveStatus.sessionHealth as the single source of
// truth for the closed statuses (done/archived/stopped/idle/closed — including
// the literal "closed" the Rust backend writes in mark_agent_session_closed).
// The explicit literal check below is now REDUNDANT (sessionHealth covers it)
// but kept as a harmless belt-and-suspenders guard so this stays correct even
// if the closed-status vocabulary in agentLiveStatus.ts ever drifts.
function isClosed(session: AgentSession, nowMs: number): boolean {
  if (session.status.toLowerCase() === "closed") return true;
  return sessionHealth(session, nowMs) === "closed";
}

const ROLE_ORDER: Record<string, number> = {
  orchestrator: 0,
  coder: 1,
  verifier: 2,
};

const MODEL_ORDER: Record<string, number> = {
  opus: 0,
  sonnet: 1,
  haiku: 2,
};

function rankRole(role: string): number {
  const r = ROLE_ORDER[role.toLowerCase()];
  return r === undefined ? Number.MAX_SAFE_INTEGER : r;
}

function rankModel(model: string): number {
  const m = MODEL_ORDER[model.toLowerCase()];
  return m === undefined ? Number.MAX_SAFE_INTEGER : m;
}

// Fold the live fleet into stable per-(role, model) counts.
//
// Each NON-closed session contributes +1 as (session.role, session.model ||
// "unknown"). Then each of that session's subagents entries contributes
// +entry.count as (entry.role ?? session.role, entry.model || "unknown").
// Closed sessions are skipped entirely (their subagents too).
export function fleetCounts(
  sessions: AgentSession[],
  nowMs: number = Date.now(),
): FleetCount[] {
  // Keyed by JSON.stringify([role, model]) so a role/model containing the
  // delimiter (arbitrary wire strings) can never collide two distinct pairs.
  const counts = new Map<string, FleetCount>();

  const bump = (role: string, modelRaw: string | null | undefined, by: number) => {
    if (by <= 0) return;
    const model = modelRaw && modelRaw.length > 0 ? modelRaw : MODEL_UNKNOWN;
    const key = JSON.stringify([role, model]);
    const existing = counts.get(key);
    if (existing) {
      existing.count += by;
    } else {
      counts.set(key, { role, model, count: by });
    }
  };

  for (const session of sessions) {
    if (isClosed(session, nowMs)) continue;
    bump(session.role, session.model, 1);
    for (const sub of session.subagents ?? []) {
      bump(sub.role ?? session.role, sub.model, sub.count);
    }
  }

  return [...counts.values()].sort((a, b) => {
    const roleDelta = rankRole(a.role) - rankRole(b.role);
    if (roleDelta !== 0) return roleDelta;
    if (a.role !== b.role) return a.role.localeCompare(b.role);
    const modelDelta = rankModel(a.model) - rankModel(b.model);
    if (modelDelta !== 0) return modelDelta;
    return a.model.localeCompare(b.model);
  });
}

function pluralizeRole(role: string, count: number): string {
  return count === 1 ? role : `${role}s`;
}

// Render the counts as "1 opus orchestrator · 2 opus coders · 6 sonnet
// reviewers" — model then role, role naively pluralized with a trailing "s".
// Returns "" for an empty fleet so callers can decide whether to render.
export function summarizeFleet(counts: FleetCount[]): string {
  return counts
    .map(
      (c) => `${c.count} ${c.model} ${pluralizeRole(c.role, c.count)}`,
    )
    .join(" · ");
}

// Total headcount of self-reported subagents across the live (non-closed) fleet.
// These are advisory: an agent reports its own fan-out over MCP, so the fleet
// counts/headline include numbers the app cannot independently verify. Closed
// sessions (and their subagents) are excluded, matching fleetCounts.
export function reportedSubagentTotal(
  sessions: AgentSession[],
  nowMs: number = Date.now(),
): number {
  let total = 0;
  for (const session of sessions) {
    if (isClosed(session, nowMs)) continue;
    for (const sub of session.subagents ?? []) {
      if (sub.count > 0) total += sub.count;
    }
  }
  return total;
}

// Muted suffix appended to the fleet headline when the counts include any
// self-reported subagents, so the number is not mistaken for a verified process
// count. Empty string when no subagents are reported (nothing to disclaim).
export function fleetHeadlineSuffix(
  sessions: AgentSession[],
  nowMs: number = Date.now(),
): string {
  return reportedSubagentTotal(sessions, nowMs) > 0
    ? " (incl. reported subagents)"
    : "";
}

// Sessions that need a human: explicitly flagged needsUser, or unhealthy in a
// way that needs recovery (stale/lost heartbeat). Closed sessions are excluded
// FIRST — a done/closed session can still carry a leftover needsUser written
// before it closed, and that must not ring the bell. Order is preserved.
export function attentionSessions(
  sessions: AgentSession[],
  nowMs: number = Date.now(),
): AgentSession[] {
  return sessions.filter((session) => {
    if (isClosed(session, nowMs)) return false;
    if (session.needsUser) return true;
    const health = sessionHealth(session, nowMs);
    return health === "stale" || health === "lost";
  });
}
