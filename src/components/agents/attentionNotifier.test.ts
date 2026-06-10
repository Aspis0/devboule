import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  shouldNotify,
  startAttentionWatcher,
  buildNotificationBody,
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
});

describe("startAttentionWatcher singleton guard (#4)", () => {
  // Minimal mock matching the slice of the zustand store the watcher uses:
  // getState().sessions + subscribe(). subscribe records how many live listeners
  // exist so we can prove StrictMode's double-start creates only one.
  function mockStore() {
    const listeners = new Set<(state: { sessions: AgentSession[] }) => void>();
    const store = {
      getState: () => ({ sessions: [] as AgentSession[] }),
      subscribe: (fn: (state: { sessions: AgentSession[] }) => void) => {
        listeners.add(fn);
        return () => listeners.delete(fn);
      },
    };
    return { store: store as unknown as AgentAttentionStore, listeners };
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
