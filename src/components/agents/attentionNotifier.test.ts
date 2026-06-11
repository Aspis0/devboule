import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  shouldNotify,
  notificationDecision,
  startAttentionWatcher,
  buildNotificationBody,
  buildSummaryNotificationBody,
  buildOutcomeNotificationBody,
  stripSpoofChars,
  notifyAgentsNeedYou,
  NOTIFICATION_BODY_MAX,
  NOTIFICATION_PER_MINUTE_CAP,
  type ShouldNotifyDeps,
} from "./attentionNotifier";
import type { AgentSession } from "../../types/backend";
import type { AgentAttentionStore } from "../../store/agentAttentionStore";

// Pure-logic tests for the OS-notification decision. The Tauri plugin wrapper
// (notifyAgentsNeedYou) and the store glue (startAttentionWatcher) are exercised
// only indirectly — the dedup + cap rules are the contract worth pinning.

// Mock the lazily-imported notification plugin so we can control when the
// permission request resolves (to simulate a prompt that resolves AFTER the
// watcher has been torn down — see the "cancelled" test).
const sendNotificationMock = vi.fn();
let permissionResolve: (granted: boolean) => void;
let permissionPromise: Promise<boolean>;
vi.mock("@tauri-apps/plugin-notification", () => ({
  isPermissionGranted: () => Promise.resolve(false),
  requestPermission: () =>
    permissionPromise.then((g) => (g ? "granted" : "denied")),
  sendNotification: (...args: unknown[]) => sendNotificationMock(...args),
}));

function session(overrides: Partial<AgentSession>): AgentSession {
  return {
    agentId: "a-1",
    role: "coder",
    model: "sonnet",
    status: "needs_user",
    message: null,
    currentProjectId: null,
    currentTaskId: null,
    firstSeenAt: null,
    lastSeenAt: null,
    ...overrides,
  };
}

function deps(now: () => number): ShouldNotifyDeps {
  return { prevSinceByAgent: new Map(), recentFiresMs: [], now };
}

const SINCE_A = "2026-06-04T10:00:00.000Z";
const SINCE_B = "2026-06-04T10:05:00.000Z";

async function flushAsync(turns = 5): Promise<void> {
  for (let i = 0; i < turns; i += 1) await Promise.resolve();
}

describe("shouldNotify", () => {
  it("fires on ENTER (first time a needsUser.since is seen)", () => {
    const d = deps(() => 0);
    const s = session({ needsUser: { reason: "needs_user", message: "Approve?", since: SINCE_A } });
    expect(shouldNotify(s, d)).toBe(true);
    expect(d.prevSinceByAgent.get("a-1")).toBe(SINCE_A);
  });

  it("does NOT fire on a repeat tick with the same since", () => {
    const d = deps(() => 0);
    const s = session({ needsUser: { reason: "needs_user", message: "Approve?", since: SINCE_A } });
    expect(shouldNotify(s, d)).toBe(true);
    expect(shouldNotify(s, d)).toBe(false);
    expect(shouldNotify(s, d)).toBe(false);
  });

  it("re-fires when since changes (a fresh re-raise)", () => {
    let t = 0;
    const d = deps(() => t);
    const s1 = session({ needsUser: { reason: "needs_user", message: "q1", since: SINCE_A } });
    expect(shouldNotify(s1, d)).toBe(true);
    t = 1000;
    const s2 = session({ needsUser: { reason: "needs_user", message: "q2", since: SINCE_B } });
    expect(shouldNotify(s2, d)).toBe(true);
    expect(d.prevSinceByAgent.get("a-1")).toBe(SINCE_B);
  });

  it("returns false for a session without needsUser (and leaves tracking)", () => {
    const d = deps(() => 0);
    d.prevSinceByAgent.set("a-1", SINCE_A);
    expect(shouldNotify(session({ needsUser: null }), d)).toBe(false);
    // shouldNotify does NOT prune on its own — leave-tracking is the watcher's job.
    expect(d.prevSinceByAgent.get("a-1")).toBe(SINCE_A);
  });

  it("returns false when since is blank", () => {
    const d = deps(() => 0);
    expect(
      shouldNotify(
        session({ needsUser: { reason: "needs_user", message: "x", since: "   " } }),
        d,
      ),
    ).toBe(false);
  });

  it("enforces the per-minute cap across distinct agents", () => {
    let t = 1_000_000;
    const d = deps(() => t);
    // Fire CAP distinct agents (each a fresh enter) -> all allowed.
    for (let i = 0; i < NOTIFICATION_PER_MINUTE_CAP; i += 1) {
      const s = session({
        agentId: `agent-${i}`,
        needsUser: { reason: "needs_user", message: "q", since: `${SINCE_A}#${i}` },
      });
      expect(shouldNotify(s, d)).toBe(true);
    }
    // One more within the same minute -> capped.
    const overflow = session({
      agentId: "agent-overflow",
      needsUser: { reason: "needs_user", message: "q", since: `${SINCE_A}#x` },
    });
    expect(shouldNotify(overflow, d)).toBe(false);
    // Capped decision must NOT record the since, so it can fire later.
    expect(d.prevSinceByAgent.has("agent-overflow")).toBe(false);

    // Advance past the rolling 60s window -> the cap frees up.
    t += 61_000;
    expect(shouldNotify(overflow, d)).toBe(true);
  });

  it("exposes capped as a decision reason", () => {
    const d = deps(() => 1_000);
    d.recentFiresMs.push(...Array.from({ length: NOTIFICATION_PER_MINUTE_CAP }, (_, i) => i));
    const s = session({
      agentId: "agent-overflow",
      needsUser: { reason: "needs_user", message: "q", since: SINCE_A },
    });
    expect(notificationDecision(s, d)).toBe("capped");
  });
});

describe("startAttentionWatcher singleton guard (#4)", () => {
  // Minimal mock matching the slice of the zustand store the watcher uses:
  // getState().sessions + subscribe(). subscribe records how many live listeners
  // exist so we can prove StrictMode's double-start creates only one.
  function mockStore() {
    const listeners = new Set<(state: { sessions: AgentSession[] }) => void>();
    let current: AgentSession[] = [];
    const store = {
      getState: () => ({ sessions: current }),
      subscribe: (fn: (state: { sessions: AgentSession[] }) => void) => {
        listeners.add(fn);
        return () => listeners.delete(fn);
      },
    };
    const emit = (sessions: AgentSession[]) => {
      current = sessions;
      for (const listener of listeners) listener({ sessions });
    };
    return { store: store as unknown as AgentAttentionStore, listeners, emit };
  }

  it("a second concurrent start is a no-op (only one subscription)", () => {
    const { store, listeners } = mockStore();
    const teardownA = startAttentionWatcher(store);
    expect(listeners.size).toBe(1);
    // StrictMode's second invocation while the first is still active: no new sub.
    const teardownB = startAttentionWatcher(store);
    expect(listeners.size).toBe(1);
    // The duplicate's teardown must NOT tear down the real subscription.
    teardownB();
    expect(listeners.size).toBe(1);
    teardownA();
    expect(listeners.size).toBe(0);
  });

  it("teardown resets the guard so a later start subscribes again", () => {
    const { store, listeners } = mockStore();
    const teardown = startAttentionWatcher(store);
    expect(listeners.size).toBe(1);
    teardown();
    expect(listeners.size).toBe(0);
    // After a clean teardown (e.g. lock→unlock), a fresh start works.
    const restart = startAttentionWatcher(store);
    expect(listeners.size).toBe(1);
    restart();
    expect(listeners.size).toBe(0);
  });

  it("loads persisted since values before notifying old needs-user requests", async () => {
    sendNotificationMock.mockClear();
    permissionPromise = Promise.resolve(true);
    const { store, emit } = mockStore();
    emit([
      session({
        needsUser: { reason: "needs_user", message: "old", since: SINCE_A },
      }),
    ]);
    const teardown = startAttentionWatcher(store, {
      loadPrevSinceByAgent: async () => ({ "a-1": SINCE_A }),
      savePrevSinceByAgent: async () => {},
    });

    await flushAsync();
    expect(sendNotificationMock).not.toHaveBeenCalled();
    teardown();
  });

  it("schedules one coalesced summary when the cap suppresses needs-user toasts", async () => {
    sendNotificationMock.mockClear();
    permissionPromise = Promise.resolve(true);
    const { store, emit } = mockStore();
    const notifySummary = vi.fn(
      async (_count: number, _isCancelled?: () => boolean) => {},
    );
    let nowMs = 1_000;
    let scheduled: (() => void) | null = null;
    let scheduleCount = 0;
    const teardown = startAttentionWatcher(store, {
      now: () => nowMs,
      recentFiresMs: Array.from({ length: NOTIFICATION_PER_MINUTE_CAP }, (_, i) => 1_000 + i),
      loadPrevSinceByAgent: async () => ({}),
      savePrevSinceByAgent: async () => {},
      notifySummary,
      setTimeoutFn: ((fn: () => void) => {
        scheduleCount += 1;
        scheduled = fn;
        return 1 as unknown as ReturnType<typeof setTimeout>;
      }) as typeof setTimeout,
      clearTimeoutFn: (() => {}) as typeof clearTimeout,
    });
    await flushAsync();
    emit(
      Array.from({ length: 3 }, (_, i) =>
        session({
          agentId: `blocked-${i}`,
          lastSeenAt: new Date(0).toISOString(),
          needsUser: {
            reason: "needs_user",
            message: "q",
            since: `${SINCE_A}-${i}`,
          },
        }),
      ),
    );

    expect(scheduleCount).toBe(1);
    nowMs = 62_000;
    const runScheduled = scheduled;
    expect(runScheduled).not.toBeNull();
    (runScheduled as unknown as () => void)();
    await flushAsync();
    expect(notifySummary).toHaveBeenCalledWith(3, expect.any(Function));
    expect(sendNotificationMock).not.toHaveBeenCalled();
    teardown();
  });

  it("notifies when an app-hosted agent reaches a terminal outcome", async () => {
    sendNotificationMock.mockClear();
    permissionPromise = Promise.resolve(true);
    const { store, emit } = mockStore();
    const notifyOutcomes = vi.fn(
      async (_sessions: AgentSession[], _isCancelled?: () => boolean) => {},
    );
    const teardown = startAttentionWatcher(store, {
      loadPrevSinceByAgent: async () => ({}),
      savePrevSinceByAgent: async () => {},
      notifyOutcomes,
    });
    await flushAsync();
    emit([
      session({
        agentId: "workflow-release",
        host: "app",
        status: "wip",
        lastSeenAt: new Date().toISOString(),
      }),
    ]);
    emit([
      session({
        agentId: "workflow-release",
        host: "app",
        status: "done",
        lastSeenAt: new Date().toISOString(),
      }),
    ]);
    await flushAsync();

    expect(notifyOutcomes).toHaveBeenCalledTimes(1);
    const outcomeSessions = notifyOutcomes.mock.calls[0]?.[0] ?? [];
    expect(outcomeSessions.map((s) => s.agentId)).toEqual([
      "workflow-release",
    ]);
    expect(sendNotificationMock).not.toHaveBeenCalled();
    teardown();
  });

  it("does NOT consume a notification slot for a terminal outcome after teardown (#MINOR4)", async () => {
    // MINOR 4 regression: reserveNotificationSlot mutates the rolling recentFiresMs budget.
    // If the watcher is torn down (cancelled) before the outcome toast actually fires, the
    // slot would be wasted. The handle() pass must skip the reservation when cancelled.
    sendNotificationMock.mockClear();
    permissionPromise = Promise.resolve(true);
    const { store, listeners } = mockStore();
    const notifyOutcomes = vi.fn(
      async (_sessions: AgentSession[], _isCancelled?: () => boolean) => {},
    );
    const teardown = startAttentionWatcher(store, {
      loadPrevSinceByAgent: async () => ({}),
      savePrevSinceByAgent: async () => {},
      notifyOutcomes,
    });
    await flushAsync();
    // Capture the live listener so we can drive a handle() pass AFTER teardown removes it
    // from the store's subscriber set (simulating a pass racing an in-flight teardown).
    const listener = [...listeners][0];
    expect(listener).toBeDefined();
    // Seed a non-terminal previous status for the agent while still live.
    listener?.({
      sessions: [
        session({ agentId: "workflow-x", host: "app", status: "wip" }),
      ],
    });
    notifyOutcomes.mockClear();
    // Tear down (sets cancelled = true), then drive a terminal transition through the
    // retained listener: the cancelled guard must prevent any slot reservation / toast.
    teardown();
    listener?.({
      sessions: [
        session({ agentId: "workflow-x", host: "app", status: "done" }),
      ],
    });
    await flushAsync();
    expect(notifyOutcomes).not.toHaveBeenCalled();
  });
});

describe("buildNotificationBody", () => {
  it("formats as `<agentId>: <message>`", () => {
    const body = buildNotificationBody(
      session({ agentId: "coder-7", needsUser: { reason: "needs_user", message: "Approve deploy?", since: SINCE_A } }),
    );
    expect(body).toBe("coder-7: Approve deploy?");
  });

  it("falls back to just the agentId when message is blank", () => {
    const body = buildNotificationBody(
      session({ agentId: "coder-7", needsUser: { reason: "needs_user", message: "  ", since: SINCE_A } }),
    );
    expect(body).toBe("coder-7");
  });

  it("caps the body length", () => {
    const body = buildNotificationBody(
      session({
        agentId: "x",
        needsUser: { reason: "needs_user", message: "z".repeat(500), since: SINCE_A },
      }),
    );
    expect(body.length).toBeLessThanOrEqual(NOTIFICATION_BODY_MAX);
    expect(body.endsWith("…")).toBe(true);
  });

  it("strips bidi/zero-width spoofing chars from agentId and message", () => {
    // RTL override + zero-width chars embedded in both the id and the message
    // (a malicious agent could use these to reorder/hide visible text).
    const body = buildNotificationBody(
      session({
        agentId: "cod‮er-7",
        needsUser: {
          reason: "needs_user",
          message: "Appr​ove⁦ dep﻿loy?",
          since: SINCE_A,
        },
      }),
    );
    expect(body).toBe("coder-7: Approve deploy?");
    // No residual bidi/zero-width code points survive.
    expect(/[​-‏‪-‮⁦-⁩﻿]/.test(body)).toBe(false);
  });
});

describe("summary and outcome notification bodies", () => {
  it("formats summary and outcome bodies", () => {
    expect(buildSummaryNotificationBody(2)).toBe("2 agents need you");
    expect(
      buildOutcomeNotificationBody(
        session({ agentId: "coder-1", host: "app", status: "failed" }),
      ),
    ).toBe("coder-1: failed");
  });
});

describe("stripSpoofChars", () => {
  it("removes bidi controls, zero-width chars and the BOM", () => {
    const dirty =
      "a​b‎c‏d‪e‫f‬g‭h‮i⁦j⁧k⁨l⁩m﻿n";
    expect(stripSpoofChars(dirty)).toBe("abcdefghijklmn");
  });

  it("is a no-op for clean text", () => {
    expect(stripSpoofChars("Approve deploy?")).toBe("Approve deploy?");
  });

  it("tolerates null/undefined", () => {
    expect(stripSpoofChars(null)).toBe("");
    expect(stripSpoofChars(undefined)).toBe("");
  });
});

describe("notifyAgentsNeedYou cancellation (#6)", () => {
  beforeEach(() => {
    sendNotificationMock.mockClear();
    permissionPromise = new Promise<boolean>((resolve) => {
      permissionResolve = resolve;
    });
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  function needsSession(): AgentSession {
    return session({
      agentId: "blocked",
      needsUser: { reason: "needs_user", message: "Approve?", since: SINCE_A },
    });
  }

  it("does NOT send when isCancelled() becomes true before the permission resolves", async () => {
    let cancelled = false;
    const promise = notifyAgentsNeedYou([needsSession()], () => cancelled);
    // The watcher tears down (e.g. app lock) while the OS permission prompt is
    // still pending: flip the cancel flag, THEN let the prompt resolve granted.
    cancelled = true;
    permissionResolve(true);
    await promise;
    expect(sendNotificationMock).not.toHaveBeenCalled();
  });

  it("sends when not cancelled and permission is granted", async () => {
    const promise = notifyAgentsNeedYou([needsSession()], () => false);
    permissionResolve(true);
    await promise;
    expect(sendNotificationMock).toHaveBeenCalledTimes(1);
  });
});
