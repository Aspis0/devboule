// Shared live-status classification for agent sessions.
//
// Extracted from AgentsView so the Projects ProjectAgentPanel and the Agents
// control room agree on the SAME heartbeat thresholds and the SAME health
// vocabulary. Keeping it in one place means a threshold change can never drift
// between the two surfaces.

import type { AgentSession } from "../../types/backend";

// Heartbeat age thresholds (mirror the copy in AgentsView's "Reconnect rules"):
// - stale after ~3 min of silence
// - reconnect/lost after ~10 min
// - a launch_pending session that never registered is stale after ~2 min
export const HEARTBEAT_STALE_MS = 3 * 60 * 1000;
export const HEARTBEAT_LOST_MS = 10 * 60 * 1000;
export const LAUNCH_PENDING_STALE_MS = 2 * 60 * 1000;

export type SessionHealth =
  | "online"
  | "pending"
  | "stale"
  | "lost"
  | "closed"
  | "unknown";

// Coarser three-state bucket the Projects panel renders as a dot + word.
export type LiveStatusTone = "working" | "idle" | "stalled";

export function parseTimestamp(value: string | null | undefined): number | null {
  if (!value) return null;
  const parsed = Date.parse(value);
  return Number.isNaN(parsed) ? null : parsed;
}

export function sessionLastSeenTime(session: AgentSession): number | null {
  return parseTimestamp(session.lastSeenAt);
}

// The single most relevant session to surface for a project across the board
// card, the status header, and the detail panel — so they never disagree about
// "who is working this". Prefers the newest lastSeenAt heartbeat, falling back
// to firstSeenAt, and intentionally includes launch_pending sessions (a freshly
// launched agent that has not heartbeat yet is still the relevant one to show).
export function freshestSession(
  sessions: AgentSession[],
): AgentSession | null {
  let best: AgentSession | null = null;
  let bestTime = -Infinity;
  for (const session of sessions) {
    const time =
      parseTimestamp(session.lastSeenAt) ??
      parseTimestamp(session.firstSeenAt) ??
      -Infinity;
    if (best === null || time > bestTime) {
      best = session;
      bestTime = time;
    }
  }
  return best;
}

// Best reference time for "how long since we heard from this agent": heartbeat,
// else launch-token issue time, else first-seen.
export function sessionReferenceTime(session: AgentSession): number | null {
  return (
    sessionLastSeenTime(session) ??
    parseTimestamp(session.launchTokenIssuedAt) ??
    parseTimestamp(session.firstSeenAt)
  );
}

export function sessionAgeMs(
  session: AgentSession,
  now = Date.now(),
): number | null {
  const referenceTime = sessionReferenceTime(session);
  return referenceTime === null ? null : now - referenceTime;
}

// Same logic AgentsView used: status keywords first, then heartbeat age.
export function sessionHealth(
  session: AgentSession,
  now = Date.now(),
): SessionHealth {
  const status = session.status.toLowerCase();
  const ageMs = sessionAgeMs(session, now);

  if (
    status === "done" ||
    status === "archived" ||
    status === "stopped" ||
    status === "idle" ||
    // Literal "closed" written by Rust mark_agent_session_closed: a just-stopped
    // agent must be treated as closed here too, otherwise it counts as "online"
    // in fleetHealthRollup until its heartbeat goes stale (~3 min).
    status === "closed"
  ) {
    return "closed";
  }
  if (status === "launch_pending") {
    return ageMs !== null && ageMs > LAUNCH_PENDING_STALE_MS
      ? "stale"
      : "pending";
  }
  if (sessionLastSeenTime(session) === null) return "unknown";
  if (ageMs !== null && ageMs > HEARTBEAT_LOST_MS) return "lost";
  if (ageMs !== null && ageMs > HEARTBEAT_STALE_MS) return "stale";
  return "online";
}

// Fold the six-state health into the three tones the Projects panel shows.
export function healthTone(health: SessionHealth): LiveStatusTone {
  switch (health) {
    case "online":
      return "working";
    case "pending":
      return "idle";
    case "stale":
    case "lost":
    case "unknown":
    case "closed":
      return "stalled";
  }
}

// One word for the live dot. "launching" is shown for a fresh launch_pending so
// the user understands the agent is still booting, not idle by choice.
export function healthWord(health: SessionHealth): string {
  switch (health) {
    case "online":
      return "working";
    case "pending":
      return "launching";
    case "stale":
      return "stalled";
    case "lost":
      return "reconnect";
    case "unknown":
      return "stalled";
    case "closed":
      return "idle";
  }
}

export function needsRecovery(health: SessionHealth): boolean {
  return health === "stale" || health === "lost" || health === "unknown";
}

// Human-friendly relative age, e.g. "3s ago", "4m ago", "2h ago".
export function formatHeartbeatAge(ageMs: number | null): string {
  if (ageMs === null || ageMs < 0) return "unknown";
  const seconds = Math.floor(ageMs / 1000);
  if (seconds < 60) return `${seconds}s ago`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 48) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}

// CLI badge metadata for the row chip, parameterized by session.client.
export interface CliBadge {
  label: string;
  toneClass: string;
}

export function cliBadge(client: string | null | undefined): CliBadge {
  const normalized = (client ?? "").toLowerCase();
  switch (normalized) {
    case "codex":
      return { label: "Codex", toneClass: "bg-terracotta/10 text-terracotta" };
    case "claude":
      return { label: "Claude", toneClass: "bg-teal/10 text-teal" };
    case "powershell":
      return { label: "PowerShell", toneClass: "bg-sage/10 text-sage-dark" };
    default:
      // A configured custom client id (or any unknown CLI): echo the id itself
      // (capitalized, capped) so the badge is meaningful rather than a generic
      // "CLI?"; fall back to "CLI?" only when truly empty/unknown.
      if (normalized.length > 0) {
        const label =
          normalized.charAt(0).toUpperCase() + normalized.slice(1, 16);
        return { label, toneClass: "bg-cream-100 text-cream-600" };
      }
      return { label: "CLI?", toneClass: "bg-cream-100 text-cream-500" };
  }
}
