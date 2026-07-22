import { describe, it, expect } from "vitest";
import {
  isRecentProjectSession,
  isActiveProjectSession,
  isLiveWorkingSession,
  clearVerifierKeysOnLeaveReview,
} from "./agentClaims";
import type { AgentSession } from "../types/backend";

const NOW = Date.parse("2026-06-06T12:00:00Z");

function session(overrides: Partial<AgentSession>): AgentSession {
  return {
    agentId: "a1",
    role: "coder",
    model: null,
    status: "active",
    message: null,
    currentProjectId: "p1",
    currentTaskId: null,
    firstSeenAt: null,
    // 1 minute ago — well within the 15-min activity window.
    lastSeenAt: new Date(NOW - 60_000).toISOString(),
    ...overrides,
  };
}

describe("isRecentProjectSession", () => {
  it("includes a fresh active session with a project", () => {
    expect(isRecentProjectSession(session({}), NOW)).toBe(true);
  });

  it("excludes a 'closed' session (PTY-EOF terminal status) — the carry-over fix", () => {
    // A mini reaped by the PTY reader writes status "closed". Before the fix it
    // re-appeared in the rail until its 15-min window expired.
    expect(isRecentProjectSession(session({ status: "closed" }), NOW)).toBe(false);
  });

  it("still excludes the other terminal statuses (regression)", () => {
    for (const status of ["done", "archived", "idle", "stopped"]) {
      expect(isRecentProjectSession(session({ status }), NOW)).toBe(false);
    }
  });

  it("excludes a session with no project", () => {
    expect(isRecentProjectSession(session({ currentProjectId: null }), NOW)).toBe(
      false,
    );
  });

  it("excludes a stale session outside the activity window", () => {
    const stale = session({
      lastSeenAt: new Date(NOW - 20 * 60_000).toISOString(),
    });
    expect(isRecentProjectSession(stale, NOW)).toBe(false);
  });
});

describe("isActiveProjectSession", () => {
  it("a 'closed' session is not active either (built on isRecentProjectSession)", () => {
    expect(isActiveProjectSession(session({ status: "closed" }), NOW)).toBe(false);
  });
});

describe("isLiveWorkingSession (F43)", () => {
  it("includes a fresh active heartbeat", () => {
    expect(isLiveWorkingSession(session({}), NOW)).toBe(true);
  });

  it("excludes ghost active sessions with stale heartbeat (pre-fix hang)", () => {
    // Within 15-min "recent" window but past 3-min stale threshold → ghost.
    const ghost = session({
      status: "active",
      lastSeenAt: new Date(NOW - 10 * 60_000).toISOString(),
    });
    expect(isRecentProjectSession(ghost, NOW)).toBe(true);
    expect(isLiveWorkingSession(ghost, NOW)).toBe(false);
  });

  it("excludes stale launch_pending (never registered)", () => {
    const pending = session({
      status: "launch_pending",
      lastSeenAt: null,
      firstSeenAt: new Date(NOW - 5 * 60_000).toISOString(),
    });
    expect(isLiveWorkingSession(pending, NOW)).toBe(false);
  });

  it("includes fresh launch_pending", () => {
    const pending = session({
      status: "launch_pending",
      lastSeenAt: null,
      firstSeenAt: new Date(NOW - 30_000).toISOString(),
    });
    expect(isLiveWorkingSession(pending, NOW)).toBe(true);
  });
});

describe("clearVerifierKeysOnLeaveReview (F35)", () => {
  it("clears fired + failed so re-entry can re-verify", () => {
    const fired = new Set(["p1:T1", "p1:T2"]);
    const failed = new Set(["p1:T1"]);
    const changed = clearVerifierKeysOnLeaveReview(fired, failed, "p1:T1");
    expect(changed).toBe(true);
    expect(fired.has("p1:T1")).toBe(false);
    expect(failed.has("p1:T1")).toBe(false);
    expect(fired.has("p1:T2")).toBe(true);
  });

  it("returns false when key was not in fired (no persist needed)", () => {
    const fired = new Set<string>();
    const failed = new Set(["p1:T1"]);
    expect(clearVerifierKeysOnLeaveReview(fired, failed, "p1:T1")).toBe(false);
    expect(failed.has("p1:T1")).toBe(false);
  });
});
