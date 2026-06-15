// @vitest-environment jsdom
//
// Unit tests for the console transport: the pure incremental reducer
// (applyMiniActivityEvent) + the channel name, PLUS the two effect-wiring guarantees
// that are easiest to pin here with injected deps and a controlled async timeline:
//   - buffer-and-replay: an event delivered in the subscribe→snapshot window is
//     preserved, not clobbered by the snapshot apply (FIX 1).
//   - cleanup on agentId CHANGE: switching agentId unlistens the prior channel and
//     ignores the prior agent's late events (FIX 6).
// The reducer tests are pure and would also pass in node; the hook tests need jsdom,
// so the whole file runs under jsdom. Broader render/degradation cases live in
// AgentConsole.test.tsx.

import { describe, it, expect, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import type { ConsoleActivity } from "./agentConsoleModel";
import {
  type AgentConsoleDeps,
  type MiniActivityEvent,
  type UnlistenFn,
  applyMiniActivityEvent,
  miniActivityChannel,
  useAgentConsole,
} from "./useAgentConsole";

(
  globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

function spawnState(): ConsoleActivity {
  return {
    running: true,
    runCount: 1,
    entries: [
      {
        type: "spawn",
        text: "spawned mini-coder",
        time: "14:22:08",
        mini: {
          model: "mini · sonnet-4",
          scope: ["auth.rs"],
          rounds: [{ n: 1, actions: [] }],
        },
      },
    ],
  };
}

describe("miniActivityChannel", () => {
  it("builds the per-agent channel name", () => {
    expect(miniActivityChannel("coder-7")).toBe("mini-activity://coder-7");
  });
});

describe("applyMiniActivityEvent", () => {
  it("snapshot replaces the whole state", () => {
    const next = applyMiniActivityEvent(
      { empty: true },
      { type: "snapshot", activity: spawnState() },
    );
    expect(next.entries).toHaveLength(1);
    expect(next.running).toBe(true);
  });

  it("an empty snapshot normalizes to the resting state", () => {
    const next = applyMiniActivityEvent(spawnState(), {
      type: "snapshot",
      activity: {},
    });
    expect(next).toEqual({ empty: true });
  });

  it("appendEntry pushes a row and clears empty", () => {
    const next = applyMiniActivityEvent(
      { empty: true },
      {
        type: "appendEntry",
        entry: { type: "coder", text: "claimed", time: "00:00" },
      },
    );
    expect(next.empty).toBe(false);
    expect(next.entries).toHaveLength(1);
  });

  it("appendRound appends to the last mini run", () => {
    const next = applyMiniActivityEvent(spawnState(), {
      type: "appendRound",
      round: { n: 2, actions: [] },
    });
    const entry = next.entries?.[0];
    expect(entry?.type).toBe("spawn");
    if (entry?.type === "spawn") {
      expect(entry.mini.rounds).toHaveLength(2);
      expect(entry.mini.rounds[1].n).toBe(2);
    }
  });

  it("appendAction targets a round by index", () => {
    const next = applyMiniActivityEvent(spawnState(), {
      type: "appendAction",
      roundIndex: 0,
      action: { kind: "read", verb: "Read", target: "src/auth.rs", ok: true },
    });
    const entry = next.entries?.[0];
    if (entry?.type === "spawn") {
      expect(entry.mini.rounds[0].actions).toHaveLength(1);
      expect(entry.mini.rounds[0].actions[0].verb).toBe("Read");
    }
  });

  it("setVerdict sets a round verdict", () => {
    const next = applyMiniActivityEvent(spawnState(), {
      type: "setVerdict",
      roundIndex: 0,
      verdict: { state: "clean", files: "1 file" },
    });
    const entry = next.entries?.[0];
    if (entry?.type === "spawn") {
      expect(entry.mini.rounds[0].verdict?.state).toBe("clean");
    }
  });

  it("setBanner / setWorking mutate the last mini run", () => {
    const banner = applyMiniActivityEvent(spawnState(), {
      type: "setBanner",
      banner: { kind: "done", sub: "1 round" },
    });
    const bEntry = banner.entries?.[0];
    if (bEntry?.type === "spawn") {
      expect(bEntry.mini.banner?.kind).toBe("done");
    }

    const cleared = applyMiniActivityEvent(spawnState(), {
      type: "setWorking",
      working: undefined,
    });
    const cEntry = cleared.entries?.[0];
    if (cEntry?.type === "spawn") {
      expect(cEntry.mini.working).toBeUndefined();
    }
  });

  it("setRunning updates the tab pill without touching the timeline", () => {
    const next = applyMiniActivityEvent(spawnState(), {
      type: "setRunning",
      running: false,
      runCount: 0,
    });
    expect(next.running).toBe(false);
    expect(next.entries).toHaveLength(1);
  });

  it("a round delta with no live mini run is a no-op", () => {
    const prev: ConsoleActivity = {
      entries: [{ type: "coder", text: "x", time: "00:00" }],
    };
    const next = applyMiniActivityEvent(prev, {
      type: "appendRound",
      round: { n: 1, actions: [] },
    });
    expect(next).toBe(prev);
  });

  it("an out-of-range roundIndex is a no-op", () => {
    const prev = spawnState();
    const next = applyMiniActivityEvent(prev, {
      type: "appendAction",
      roundIndex: 9,
      action: { kind: "read", verb: "Read" },
    });
    expect(next).toBe(prev);
  });

  it("does NOT mutate the prior activity (pure)", () => {
    const prev = spawnState();
    const snapshotBefore = JSON.stringify(prev);
    applyMiniActivityEvent(prev, {
      type: "appendRound",
      round: { n: 2, actions: [] },
    });
    expect(JSON.stringify(prev)).toBe(snapshotBefore);
  });
});

// ---- hook effect wiring (jsdom) ---------------------------------------------
//
// A tiny harness that mounts the hook and writes its current `ConsoleActivity` into a
// ref so a test can assert the live value directly (no AgentConsole render needed).

/** A manually-settleable promise, to control WHEN/HOW the snapshot settles. */
function deferred<T>(): {
  promise: Promise<T>;
  resolve: (v: T) => void;
  reject: (e: unknown) => void;
} {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe("useAgentConsole — effect wiring", () => {
  it("buffers an event arriving in the subscribe→snapshot window and replays it on top of the snapshot (FIX 1: not clobbered)", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    let emit: ((e: { payload: MiniActivityEvent }) => void) | null = null;
    const snap = deferred<ConsoleActivity>();
    let latest: ConsoleActivity = { empty: true };

    const deps: AgentConsoleDeps = {
      listen: vi.fn(async (_channel, handler) => {
        emit = handler;
        return () => {};
      }),
      // Does NOT resolve until we tell it to, so we can deliver an event in the
      // subscribe→snapshot window deterministically.
      fetchSnapshot: vi.fn(() => snap.promise),
    };

    function Harness() {
      latest = useAgentConsole("coder-1", deps);
      return null;
    }

    // Mount: subscribe resolves (listen is awaited), snapshot is still pending.
    await act(async () => {
      root.render(createElement(Harness));
    });
    expect(deps.listen).toHaveBeenCalledTimes(1);
    expect(emit).toBeTypeOf("function");

    // Deliver an event AFTER listen resolved but BEFORE the snapshot resolves. It
    // must be BUFFERED, not applied: the live value is still the resting state.
    await act(async () => {
      emit?.({
        payload: {
          type: "appendEntry",
          entry: { type: "coder", text: "buffered milestone", time: "00:01" },
        },
      });
    });
    expect(latest.entries ?? []).toHaveLength(0);

    // Now resolve the snapshot (a one-entry spawn). The hook applies it as the base
    // THEN replays the buffered event on top — final state reflects BOTH.
    await act(async () => {
      snap.resolve(spawnState());
      await snap.promise;
      await Promise.resolve();
    });

    expect(latest.entries).toHaveLength(2);
    // base (the spawn) is preserved …
    expect(latest.entries?.[0]?.type).toBe("spawn");
    // … and the buffered event is replayed on top (not clobbered).
    expect(latest.entries?.[1]?.text).toBe("buffered milestone");

    await act(async () => {
      root.unmount();
    });
    container.remove();
  });

  it("flushes the buffer onto the empty state when the snapshot THROWS (FIX 1: pre-backend live channel not lost)", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    let emit: ((e: { payload: MiniActivityEvent }) => void) | null = null;
    const snap = deferred<ConsoleActivity>();
    let latest: ConsoleActivity = { empty: true };

    const deps: AgentConsoleDeps = {
      listen: vi.fn(async (_channel, handler) => {
        emit = handler;
        return () => {};
      }),
      fetchSnapshot: vi.fn(() => snap.promise),
    };

    function Harness() {
      latest = useAgentConsole("coder-1", deps);
      return null;
    }

    await act(async () => {
      root.render(createElement(Harness));
    });

    // Buffer an event during the window …
    await act(async () => {
      emit?.({
        payload: {
          type: "appendEntry",
          entry: { type: "coder", text: "early live event", time: "00:01" },
        },
      });
    });

    // … then the snapshot fetch fails. The buffer must still flush onto emptyActivity().
    await act(async () => {
      snap.reject(new Error("no backend"));
      // Let the hook's catch + setActivity microtasks settle.
      await snap.promise.catch(() => {});
      await Promise.resolve();
    });

    expect(latest.entries).toHaveLength(1);
    expect(latest.entries?.[0]?.text).toBe("early live event");

    await act(async () => {
      root.unmount();
    });
    container.remove();
  });

  it("unlistens the prior channel and ignores its late events on an agentId CHANGE (FIX 6)", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    // Per-agent unlisten spies + the handler captured for agent "a".
    const unlistenByAgent: Record<string, ReturnType<typeof vi.fn>> = {
      a: vi.fn(),
      b: vi.fn(),
    };
    const handlerByAgent: Record<
      string,
      ((e: { payload: MiniActivityEvent }) => void) | null
    > = { a: null, b: null };

    const deps: AgentConsoleDeps = {
      listen: vi.fn(
        async (channel, handler): Promise<UnlistenFn> => {
          const agentId = channel.replace("mini-activity://", "");
          handlerByAgent[agentId] = handler;
          return unlistenByAgent[agentId];
        },
      ),
      // Resolves to empty so the timeline is driven purely by channel events.
      fetchSnapshot: vi.fn(async () => ({ empty: true }) as ConsoleActivity),
    };

    let latest: ConsoleActivity = { empty: true };
    function Harness({ agentId }: { agentId: string }) {
      latest = useAgentConsole(agentId, deps);
      return null;
    }

    // Mount on agent "a".
    await act(async () => {
      root.render(createElement(Harness, { agentId: "a" }));
    });
    expect(handlerByAgent.a).toBeTypeOf("function");
    const lateHandlerForA = handlerByAgent.a;
    expect(unlistenByAgent.a).not.toHaveBeenCalled();

    // Rerender with agentId "b": the effect cleanup must unlisten "a".
    await act(async () => {
      root.render(createElement(Harness, { agentId: "b" }));
    });
    expect(unlistenByAgent.a).toHaveBeenCalledTimes(1);
    expect(handlerByAgent.b).toBeTypeOf("function");

    // A LATE event on "a"'s old handler must be IGNORED (the `active` guard of the
    // superseded effect run is false), so it never lands in the live state.
    await act(async () => {
      lateHandlerForA?.({
        payload: {
          type: "appendEntry",
          entry: { type: "coder", text: "stale-from-a", time: "00:09" },
        },
      });
    });
    expect(JSON.stringify(latest)).not.toContain("stale-from-a");

    await act(async () => {
      root.unmount();
    });
    // Unmount tears down "b" too.
    expect(unlistenByAgent.b).toHaveBeenCalledTimes(1);
    container.remove();
  });
});
