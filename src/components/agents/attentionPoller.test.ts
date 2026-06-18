import { describe, it, expect, vi } from "vitest";
import {
  shouldAttentionTick,
  startAttentionPoller,
} from "./attentionPoller";
import type { AgentLiveState } from "../../types/backend";

// The poller's whole contract is its pure gating predicate + the single-feeder /
// in-flight guarantees. These tests pin the predicate exhaustively and assert the
// glue honours it (no double-poll, no stale feed after teardown).

describe("shouldAttentionTick (global attention poller gate)", () => {
  const base = {
    unlocked: true,
    visible: true,
    inFlight: false,
    activeView: "providers",
  };

  it("fetches when unlocked, visible, idle, and NOT on the Projects view", () => {
    expect(shouldAttentionTick(base)).toBe(true);
  });

  it("does NOT fetch while locked", () => {
    expect(shouldAttentionTick({ ...base, unlocked: false })).toBe(false);
  });

  it("does NOT fetch while the document is hidden (no background polls)", () => {
    expect(shouldAttentionTick({ ...base, visible: false })).toBe(false);
  });

  it("does NOT fetch while a previous fetch is in flight (no stacked polls)", () => {
    expect(shouldAttentionTick({ ...base, inFlight: true })).toBe(false);
  });

  it("does NOT fetch on the Projects view (ProjectsView is the feeder there)", () => {
    // This is the single-feeder invariant: exactly one get_agent_live_state at a time.
    expect(shouldAttentionTick({ ...base, activeView: "projects" })).toBe(false);
  });

  it("DOES fetch on every non-projects view", () => {
    for (const view of ["providers", "polis", "oracle", "cloudflare", "settings"]) {
      expect(shouldAttentionTick({ ...base, activeView: view })).toBe(true);
    }
  });
});

describe("startAttentionPoller live-lock gate (BLOCKER: never tick while locked)", () => {
  function lockHarness(isUnlocked: () => boolean) {
    let intervalCb: (() => void) | undefined;
    const fetchLiveState = vi.fn(async () => liveState());
    const feed = vi.fn();
    const stop = startAttentionPoller({
      getActiveView: () => "providers",
      isUnlocked,
      fetchLiveState,
      feed,
      isVisible: () => true,
      setIntervalFn: (cb) => {
        intervalCb = cb;
        return 1;
      },
      clearIntervalFn: () => {
        intervalCb = undefined;
      },
    });
    return { tick: () => intervalCb?.(), fetchLiveState, feed, stop };
  }

  it("does NOT fetch on a tick while the app is locked (live state, not teardown flag)", () => {
    const h = lockHarness(() => false); // locked right now
    h.tick();
    expect(h.fetchLiveState).not.toHaveBeenCalled();
    expect(h.feed).not.toHaveBeenCalled();
    h.stop();
  });

  it("resumes fetching once the live lock state clears", async () => {
    let locked = true;
    const h = lockHarness(() => !locked);
    h.tick();
    expect(h.fetchLiveState).not.toHaveBeenCalled();
    locked = false;
    h.tick();
    await Promise.resolve();
    await Promise.resolve();
    expect(h.fetchLiveState).toHaveBeenCalledTimes(1);
    expect(h.feed).toHaveBeenCalledTimes(1);
    h.stop();
  });

  it("does NOT feed a result that resolves after the app locks mid-fetch", async () => {
    let locked = false;
    let resolveFetch: (v: AgentLiveState) => void = () => {};
    const fetchLiveState = vi.fn(
      () =>
        new Promise<AgentLiveState>((resolve) => {
          resolveFetch = resolve;
        }),
    );
    const feed = vi.fn();
    let intervalCb: (() => void) | undefined;
    const stop = startAttentionPoller({
      getActiveView: () => "providers",
      isUnlocked: () => !locked,
      fetchLiveState,
      feed,
      isVisible: () => true,
      setIntervalFn: (cb) => {
        intervalCb = cb;
        return 1;
      },
      clearIntervalFn: () => {},
    });
    intervalCb?.();
    expect(fetchLiveState).toHaveBeenCalledTimes(1);
    // App locks WHILE the fetch is in flight; the late result must be dropped even
    // though the poller was not torn down (teardown flag still false).
    locked = true;
    resolveFetch(liveState());
    await Promise.resolve();
    await Promise.resolve();
    expect(feed).not.toHaveBeenCalled();
    stop();
  });
});

function liveState(): AgentLiveState {
  return {
    version: 2,
    updatedAt: "2026-06-06T10:00:00.000Z",
    sessions: [],
    claims: [],
    events: [],
    rules: [],
    statePath: "",
    mcpCommand: "",
    mcpClientConfig: "",
  };
}

describe("startAttentionPoller glue", () => {
  function harness(activeView: string) {
    let intervalCb: (() => void) | undefined;
    const fetchLiveState = vi.fn(async () => liveState());
    const feed = vi.fn();
    const stop = startAttentionPoller({
      getActiveView: () => activeView,
      fetchLiveState,
      feed,
      isVisible: () => true,
      setIntervalFn: (cb) => {
        intervalCb = cb;
        return 1;
      },
      clearIntervalFn: () => {
        intervalCb = undefined;
      },
    });
    return {
      tick: () => intervalCb?.(),
      fetchLiveState,
      feed,
      stop,
      isArmed: () => intervalCb !== undefined,
    };
  }

  it("fetches and feeds on a tick off the Projects view", async () => {
    const h = harness("providers");
    h.tick();
    await Promise.resolve();
    await Promise.resolve();
    expect(h.fetchLiveState).toHaveBeenCalledTimes(1);
    expect(h.feed).toHaveBeenCalledTimes(1);
    h.stop();
  });

  it("does NOT fetch on a tick while the Projects view is active (single feeder)", () => {
    const h = harness("projects");
    h.tick();
    expect(h.fetchLiveState).not.toHaveBeenCalled();
    expect(h.feed).not.toHaveBeenCalled();
    h.stop();
  });

  it("does not feed a result that resolves after teardown (no stale feed)", async () => {
    let resolveFetch: (v: AgentLiveState) => void = () => {};
    const fetchLiveState = vi.fn(
      () =>
        new Promise<AgentLiveState>((resolve) => {
          resolveFetch = resolve;
        }),
    );
    const feed = vi.fn();
    let intervalCb: (() => void) | undefined;
    const stop = startAttentionPoller({
      getActiveView: () => "providers",
      fetchLiveState,
      feed,
      isVisible: () => true,
      setIntervalFn: (cb) => {
        intervalCb = cb;
        return 1;
      },
      clearIntervalFn: () => {},
    });
    intervalCb?.();
    expect(fetchLiveState).toHaveBeenCalledTimes(1);
    // Tear down BEFORE the fetch resolves; the late result must be ignored.
    stop();
    resolveFetch(liveState());
    await Promise.resolve();
    await Promise.resolve();
    expect(feed).not.toHaveBeenCalled();
  });

  it("clears the interval on teardown", () => {
    const clearIntervalFn = vi.fn();
    const stop = startAttentionPoller({
      getActiveView: () => "providers",
      fetchLiveState: async () => liveState(),
      feed: () => {},
      isVisible: () => true,
      setIntervalFn: () => 42,
      clearIntervalFn,
    });
    stop();
    expect(clearIntervalFn).toHaveBeenCalledWith(42);
  });
});
