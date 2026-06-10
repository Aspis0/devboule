import { describe, it, expect, vi } from "vitest";
import {
  startTokenUsageTracker,
  TOKEN_USAGE_POLL_MS,
} from "./useAgentTokenUsage";
import type { AgentTokenUsage } from "../../types/backend";

// Flush enough microtask turns for a fetch's then->finally chain to settle
// (the in-flight flag is cleared in finally, one extra turn after then).
const flush = async () => {
  for (let i = 0; i < 5; i++) await Promise.resolve();
};

const claude = (total: number): AgentTokenUsage => ({
  tokens: { input: 0, output: 0, cacheCreation: 0, cacheRead: 0, total },
  costUsd: 1,
  source: "claude-transcript",
});

const unavailable = (): AgentTokenUsage => ({
  tokens: { input: 0, output: 0, cacheCreation: 0, cacheRead: 0, total: 0 },
  costUsd: null,
  source: "unavailable",
});

// A controllable interval stub: capture the callback AND the cadence so the test can
// fire ticks and assert the cadence is actually wired (a stub that ignored `ms` would
// let a wrong cadence — e.g. 0ms — pass silently).
function makeIntervalStub() {
  let cb: (() => void) | null = null;
  let cleared = false;
  let ms: number | null = null;
  return {
    setIntervalFn: (fn: () => void, intervalMs: number) => {
      cb = fn;
      ms = intervalMs;
      return 1;
    },
    clearIntervalFn: () => {
      cleared = true;
    },
    tick: () => cb?.(),
    get cleared() {
      return cleared;
    },
    get ms() {
      return ms;
    },
  };
}

describe("startTokenUsageTracker", () => {
  it("fetches once on start for the selected agent and emits the value", async () => {
    const fetchUsage = vi.fn(async () => claude(1000));
    const values: (AgentTokenUsage | null)[] = [];
    const ints = makeIntervalStub();

    const stop = startTokenUsageTracker("coder-1", {
      fetchUsage,
      onValue: (v) => values.push(v),
      setIntervalFn: ints.setIntervalFn,
      clearIntervalFn: ints.clearIntervalFn,
    });
    await Promise.resolve();
    await Promise.resolve();

    expect(fetchUsage).toHaveBeenCalledTimes(1);
    expect(fetchUsage).toHaveBeenCalledWith("coder-1");
    expect(values).toEqual([claude(1000)]);
    stop();
    expect(ints.cleared).toBe(true);
  });

  it("refreshes on the slow interval (not on a 5s live-state tick — it has its own timer)", async () => {
    const fetchUsage = vi.fn(async () => claude(1));
    const ints = makeIntervalStub();
    const stop = startTokenUsageTracker("coder-1", {
      fetchUsage,
      onValue: () => {},
      setIntervalFn: ints.setIntervalFn,
      clearIntervalFn: ints.clearIntervalFn,
    });
    await flush();
    expect(fetchUsage).toHaveBeenCalledTimes(1); // initial
    ints.tick();
    await flush();
    expect(fetchUsage).toHaveBeenCalledTimes(2); // one per its own slow tick
    stop();
  });

  it("schedules the interval at TOKEN_USAGE_POLL_MS (cadence is actually wired)", async () => {
    const fetchUsage = vi.fn(async () => claude(1));
    const ints = makeIntervalStub();
    const setIntervalSpy = vi.fn(ints.setIntervalFn);
    const stop = startTokenUsageTracker("coder-1", {
      fetchUsage,
      onValue: () => {},
      setIntervalFn: setIntervalSpy,
      clearIntervalFn: ints.clearIntervalFn,
    });
    // The interval must be set with the intended slow cadence — not 0ms / undefined.
    expect(setIntervalSpy).toHaveBeenCalledWith(
      expect.any(Function),
      TOKEN_USAGE_POLL_MS,
    );
    expect(ints.ms).toBe(TOKEN_USAGE_POLL_MS);
    stop();
  });

  it("emits null for an unavailable source (badge hides)", async () => {
    const fetchUsage = vi.fn(async () => unavailable());
    const values: (AgentTokenUsage | null)[] = [];
    const ints = makeIntervalStub();
    const stop = startTokenUsageTracker("coder-1", {
      fetchUsage,
      onValue: (v) => values.push(v),
      setIntervalFn: ints.setIntervalFn,
      clearIntervalFn: ints.clearIntervalFn,
    });
    await Promise.resolve();
    await Promise.resolve();
    expect(values).toEqual([null]);
    stop();
  });

  it("drops a fetch that resolves after teardown (no leak / no stale write)", async () => {
    const holder: { resolve?: (u: AgentTokenUsage) => void } = {};
    const fetchUsage = vi.fn(
      () =>
        new Promise<AgentTokenUsage>((res) => {
          holder.resolve = res;
        }),
    );
    const values: (AgentTokenUsage | null)[] = [];
    const ints = makeIntervalStub();
    const stop = startTokenUsageTracker("coder-1", {
      fetchUsage,
      onValue: (v) => values.push(v),
      setIntervalFn: ints.setIntervalFn,
      clearIntervalFn: ints.clearIntervalFn,
    });
    // Tear down BEFORE the in-flight fetch resolves.
    stop();
    holder.resolve?.(claude(999));
    await Promise.resolve();
    await Promise.resolve();
    // The late result must be dropped — nothing was emitted.
    expect(values).toEqual([]);
    expect(ints.cleared).toBe(true);
  });

  it("does not permanently block when the fetch throws SYNCHRONOUSLY", async () => {
    // FIX 7: a fetchUsage that throws synchronously (before returning a promise) must
    // NOT leave inFlight stuck true — every subsequent tick must still fetch. Without
    // the try/catch around the kickoff, the first throw would block the agent forever.
    let calls = 0;
    const fetchUsage = vi.fn((_id: string): Promise<AgentTokenUsage> => {
      calls += 1;
      throw new Error("sync boom");
    });
    const values: (AgentTokenUsage | null)[] = [];
    const ints = makeIntervalStub();
    const stop = startTokenUsageTracker("coder-1", {
      fetchUsage,
      onValue: (v) => values.push(v),
      setIntervalFn: ints.setIntervalFn,
      clearIntervalFn: ints.clearIntervalFn,
    });
    // The initial run threw synchronously -> emitted null, inFlight cleared.
    expect(calls).toBe(1);
    expect(values).toEqual([null]);
    // A later tick must fetch AGAIN (not blocked by a stuck inFlight).
    ints.tick();
    expect(calls).toBe(2);
    expect(values).toEqual([null, null]);
    stop();
  });

  it("recovers after a synchronous throw on a later successful tick", async () => {
    // FIX 7: once a sync-throwing fetch is replaced/recovers, a subsequent tick emits
    // the real value — proving inFlight was reset, not latched.
    let mode: "throw" | "ok" = "throw";
    const fetchUsage = vi.fn((_id: string): Promise<AgentTokenUsage> => {
      if (mode === "throw") throw new Error("sync boom");
      return Promise.resolve(claude(7));
    });
    const values: (AgentTokenUsage | null)[] = [];
    const ints = makeIntervalStub();
    const stop = startTokenUsageTracker("coder-1", {
      fetchUsage,
      onValue: (v) => values.push(v),
      setIntervalFn: ints.setIntervalFn,
      clearIntervalFn: ints.clearIntervalFn,
    });
    expect(values).toEqual([null]); // initial sync throw -> null
    mode = "ok";
    ints.tick();
    await flush();
    expect(values).toEqual([null, claude(7)]);
    stop();
  });

  it("does not stack fetches while one is in flight", async () => {
    const holder: { resolve?: (u: AgentTokenUsage) => void } = {};
    const fetchUsage = vi.fn(
      () =>
        new Promise<AgentTokenUsage>((res) => {
          holder.resolve = res;
        }),
    );
    const ints = makeIntervalStub();
    const stop = startTokenUsageTracker("coder-1", {
      fetchUsage,
      onValue: () => {},
      setIntervalFn: ints.setIntervalFn,
      clearIntervalFn: ints.clearIntervalFn,
    });
    expect(fetchUsage).toHaveBeenCalledTimes(1); // initial, still pending
    ints.tick(); // a tick while in-flight must NOT start a second fetch
    expect(fetchUsage).toHaveBeenCalledTimes(1);
    holder.resolve?.(claude(1));
    await flush();
    ints.tick(); // now that it settled, a tick fetches again
    expect(fetchUsage).toHaveBeenCalledTimes(2);
    stop();
  });
});
