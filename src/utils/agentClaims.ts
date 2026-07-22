import type { AgentClaim, AgentSession } from "../types/backend";

const ACTIVE_SESSION_WINDOW_MS = 15 * 60 * 1000;

export function claimLeaseTime(claim: AgentClaim): number | null {
  if (!claim.leaseUntil) return null;
  const leaseTime = Date.parse(claim.leaseUntil);
  return Number.isNaN(leaseTime) ? null : leaseTime;
}

export function isClaimExpired(claim: AgentClaim, now = Date.now()): boolean {
  const leaseTime = claimLeaseTime(claim);
  if (leaseTime !== null) return leaseTime <= now;
  const updatedAt = Date.parse(claim.updatedAt);
  return Number.isNaN(updatedAt) || now - updatedAt > ACTIVE_SESSION_WINDOW_MS;
}

export function isOpenClaim(claim: AgentClaim, now = Date.now()): boolean {
  return claim.status !== "done" && !isClaimExpired(claim, now);
}

export function isWorkingClaim(claim: AgentClaim, now = Date.now()): boolean {
  if (!isOpenClaim(claim, now)) return false;
  return claim.status !== "review" && claim.status !== "blocked";
}

export function sessionLastSeenTime(session: AgentSession): number | null {
  if (!session.lastSeenAt) return null;
  const lastSeen = Date.parse(session.lastSeenAt);
  return Number.isNaN(lastSeen) ? null : lastSeen;
}

export function isRecentProjectSession(session: AgentSession, now = Date.now()): boolean {
  if (!session.currentProjectId) return false;
  const status = session.status.toLowerCase();
  // "closed" is the status the PTY reader writes on EOF (the mini executor writes
  // "done"); both are terminal. Excluding only the latter let a reaped/closed agent
  // re-appear in the work-mode rail until its 15-min activity window expired.
  if (
    status === "done" ||
    status === "archived" ||
    status === "idle" ||
    status === "stopped" ||
    status === "closed"
  )
    return false;
  const lastSeen = sessionLastSeenTime(session);
  return lastSeen !== null && now - lastSeen <= ACTIVE_SESSION_WINDOW_MS;
}

export function isActiveProjectSession(session: AgentSession, now = Date.now()): boolean {
  if (!isRecentProjectSession(session, now)) return false;
  const status = session.status.toLowerCase();
  return status !== "review" && status !== "blocked" && status !== "launch_pending";
}

/** Heartbeat window for live workers (must match Rust `reap_stale_ghost_sessions`). */
export const LIVE_HEARTBEAT_STALE_MS = 3 * 60 * 1000;
/** launch_pending never-registered window (must match Rust). */
export const LAUNCH_PENDING_STALE_MS = 2 * 60 * 1000;

/**
 * F43: "agent is working this project" for board WHO + Work console must agree.
 * A ledger row with status=active but no recent heartbeat (ghost from a pre-fix
 * hang) is NOT working. Aligns with sessionHealth: only online / fresh pending.
 */
export function isLiveWorkingSession(
  session: AgentSession,
  now = Date.now(),
): boolean {
  if (!session.currentProjectId) return false;
  const status = session.status.toLowerCase();
  if (
    status === "done" ||
    status === "archived" ||
    status === "idle" ||
    status === "stopped" ||
    status === "closed" ||
    status === "review" ||
    status === "blocked"
  ) {
    return false;
  }
  const lastSeen = sessionLastSeenTime(session);
  const firstSeen = session.firstSeenAt ? Date.parse(session.firstSeenAt) : NaN;
  const ref =
    lastSeen ??
    (Number.isNaN(firstSeen) ? null : firstSeen);
  if (ref === null) return false;
  const age = now - ref;
  if (status === "launch_pending") {
    return age <= LAUNCH_PENDING_STALE_MS;
  }
  // Heartbeat must be fresh — ghosts drop out of "working" UI.
  return age <= LIVE_HEARTBEAT_STALE_MS;
}

/**
 * F35 pure helper: when a task leaves "review", clear both fired and failed
 * verifier keys so a later re-entry can spawn again. Returns whether `fired`
 * changed (caller should persist sessionStorage).
 */
export function clearVerifierKeysOnLeaveReview(
  fired: Set<string>,
  failed: Set<string>,
  key: string,
): boolean {
  failed.delete(key);
  return fired.delete(key);
}
