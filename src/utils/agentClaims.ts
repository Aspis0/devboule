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
