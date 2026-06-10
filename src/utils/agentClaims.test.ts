import { describe, it, expect } from "vitest";
import { isRecentProjectSession, isActiveProjectSession } from "./agentClaims";
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
