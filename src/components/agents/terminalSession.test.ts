import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  TerminalSession,
  terminalEventChannel,
  type TerminalEvent,
  type TerminalBanner,
  type TerminalSessionDeps,
} from "./terminalSession";
import type { TerminalViewHandle } from "./createTerminalView";

// Headless tests for the terminal viewer's data-flow controller. The repo's
// vitest runs in the node environment (no jsdom), so we never instantiate a real
// xterm or render React — the view, invoke, listen and timers are all mocked and
// the controller's sequencing is asserted directly.

interface Harness {
  session: TerminalSession;
  view: TerminalViewHandle & {
    written: string[];
    fitCount: number;
    disposeCount: number;
    /** When false, fit() reports failure (zero-size host) like the real handle. */
    fitOk: { value: boolean };
    /** Geometry the view reports; tests can force a degenerate size. */
    geometry: { cols: number; rows: number };
  };
  invoke: ReturnType<typeof vi.fn>;
  emit: (event: TerminalEvent) => void;
  unlisten: ReturnType<typeof vi.fn>;
  listenCalledWith: () => string | undefined;
  banners: TerminalBanner[];
  ctrlCStates: boolean[];
  onExited: ReturnType<typeof vi.fn>;
  /** Resolve the snapshot invoke deferred (call after subscribing). */
  resolveSnapshot: (value: string) => void;
  rejectSnapshot: (err?: unknown) => void;
  createViewError: { value: boolean };
  /** Resolve a deferred listen() (only when `deferListen` was set). */
  resolveListen: () => void;
}

function makeHarness(opts?: {
  snapshotImmediate?: string;
  snapshotRejectsImmediate?: boolean;
  /** When true, listen() stays pending until `resolveListen()` is called, so a
   *  test can dispose() while start() is awaiting the subscription. */
  deferListen?: boolean;
}): Harness {
  const written: string[] = [];
  let fitCount = 0;
  let disposeCount = 0;
  const fitOk = { value: true };
  const geometry = { cols: 80, rows: 24 };
  const view = {
    write: (d: string) => {
      written.push(d);
    },
    fit: () => {
      fitCount += 1;
      return fitOk.value;
    },
    dispose: () => {
      disposeCount += 1;
    },
    cols: () => geometry.cols,
    rows: () => geometry.rows,
    get written() {
      return written;
    },
    get fitCount() {
      return fitCount;
    },
    get disposeCount() {
      return disposeCount;
    },
    fitOk,
    geometry,
  } as Harness["view"];

  let emitFn: (event: TerminalEvent) => void = () => {};
  const unlisten = vi.fn();
  let listenChannel: string | undefined;

  // Gate so a test can hold listen() pending and dispose() mid-subscription.
  let resolveListen!: () => void;
  const listenGate = new Promise<void>((res) => {
    resolveListen = res;
  });

  // Snapshot is a deferred so tests control WHEN it resolves relative to events.
  let resolveSnapshot!: (value: string) => void;
  let rejectSnapshot!: (err?: unknown) => void;
  const snapshotPromise = new Promise<string>((res, rej) => {
    resolveSnapshot = res;
    rejectSnapshot = rej;
  });

  const banners: TerminalBanner[] = [];
  const ctrlCStates: boolean[] = [];
  const onExited = vi.fn();
  const createViewError = { value: false };

  const invoke = vi.fn(async (command: string) => {
    if (command === "agent_pty_snapshot") {
      if (opts?.snapshotRejectsImmediate) throw new Error("no terminal");
      if (opts?.snapshotImmediate !== undefined) return opts.snapshotImmediate;
      return snapshotPromise;
    }
    return undefined;
  });

  const deps: TerminalSessionDeps = {
    agentId: "coder-1",
    host: {} as HTMLElement,
    createView: async (_host, viewOpts) => {
      if (createViewError.value) throw new Error("view boom");
      // Stash onData so a test could trigger an automatic reply if needed.
      (view as unknown as { onData?: (d: string) => void }).onData =
        viewOpts.onData;
      return view;
    },
    invoke: invoke as unknown as TerminalSessionDeps["invoke"],
    listen: async (channel, handler) => {
      listenChannel = channel;
      emitFn = handler;
      if (opts?.deferListen) {
        await listenGate;
      }
      return unlisten;
    },
    onBanner: (b) => banners.push(b),
    onCtrlCArmed: (a) => ctrlCStates.push(a),
    onExited,
    setTimeout: ((fn: () => void, ms: number) =>
      setTimeout(fn, ms) as unknown as number) as TerminalSessionDeps["setTimeout"],
    clearTimeout: ((id: number) =>
      clearTimeout(id)) as TerminalSessionDeps["clearTimeout"],
  };

  const session = new TerminalSession(deps);

  return {
    session,
    view,
    invoke,
    emit: (e) => emitFn(e),
    unlisten,
    listenCalledWith: () => listenChannel,
    banners,
    ctrlCStates,
    onExited,
    resolveSnapshot,
    rejectSnapshot,
    createViewError,
    resolveListen,
  };
}

const dataEvent = (data: string): TerminalEvent => ({ payload: { data } });
const exitEvent = (): TerminalEvent => ({ payload: { exited: true } });

describe("terminalEventChannel", () => {
  it("namespaces the channel per agent (matches the Rust producer)", () => {
    expect(terminalEventChannel("coder-7f")).toBe("agent-terminal://coder-7f");
  });
});

describe("TerminalSession startup ordering (subscribe before snapshot)", () => {
  it("subscribes to the live channel before fetching the snapshot", async () => {
    const h = makeHarness({ snapshotImmediate: "SNAP" });
    await h.session.start();
    expect(h.listenCalledWith()).toBe("agent-terminal://coder-1");
    // Snapshot was written after the subscribe completed.
    expect(h.view.written).toContain("SNAP");
  });

  it("queues chunks that arrive during the snapshot fetch, then flushes them after the snapshot", async () => {
    const h = makeHarness(); // snapshot stays pending until we resolve it
    const startPromise = h.session.start();
    // Let start() reach the awaited snapshot (view built + subscribed).
    await Promise.resolve();
    await Promise.resolve();

    // A live chunk arrives WHILE the snapshot is still in flight: it must queue,
    // not be written yet.
    h.emit(dataEvent("LIVE1"));
    h.emit(dataEvent("LIVE2"));
    expect(h.view.written).toEqual([]);

    // Snapshot resolves -> snapshot written first, THEN the queued chunks in order.
    h.resolveSnapshot("SNAP");
    await startPromise;
    expect(h.view.written).toEqual(["SNAP", "LIVE1", "LIVE2"]);
  });

  it("writes live chunks directly once the snapshot is done", async () => {
    const h = makeHarness({ snapshotImmediate: "SNAP" });
    await h.session.start();
    h.emit(dataEvent("AFTER"));
    expect(h.view.written).toEqual(["SNAP", "AFTER"]);
  });

  it("shows an error banner but still goes live when the snapshot fails", async () => {
    const h = makeHarness({ snapshotRejectsImmediate: true });
    await h.session.start();
    expect(h.banners).toContainEqual({
      kind: "error",
      message: "No app terminal for this agent.",
    });
    // Live still works.
    h.emit(dataEvent("LIVE"));
    expect(h.view.written).toEqual(["LIVE"]);
  });

  it("shows an error banner when the view cannot be created and does not subscribe", async () => {
    const h = makeHarness();
    h.createViewError.value = true;
    await h.session.start();
    expect(h.banners).toContainEqual({
      kind: "error",
      message: "Could not open the terminal view.",
    });
    expect(h.listenCalledWith()).toBeUndefined();
  });
});

describe("TerminalSession exited handling", () => {
  it("sets the exited banner and calls onExited exactly once", async () => {
    const h = makeHarness({ snapshotImmediate: "" });
    await h.session.start();
    h.emit(exitEvent());
    h.emit(exitEvent()); // duplicate sentinel must be ignored
    expect(h.banners).toContainEqual({ kind: "exited" });
    expect(h.onExited).toHaveBeenCalledTimes(1);
  });

  it("ignores writes after exit", async () => {
    const h = makeHarness({ snapshotImmediate: "" });
    await h.session.start();
    h.emit(exitEvent());
    h.invoke.mockClear();
    await h.session.writeToPty("x");
    expect(h.invoke).not.toHaveBeenCalled();
  });
});

describe("TerminalSession unmount cleanup", () => {
  it("calls unlisten and dispose on the view exactly once", async () => {
    const h = makeHarness({ snapshotImmediate: "" });
    await h.session.start();
    h.session.dispose();
    h.session.dispose(); // idempotent
    expect(h.unlisten).toHaveBeenCalledTimes(1);
    expect(h.view.disposeCount).toBe(1);
  });

  it("does not act on events after dispose", async () => {
    const h = makeHarness({ snapshotImmediate: "" });
    await h.session.start();
    h.session.dispose();
    h.emit(dataEvent("LATE"));
    expect(h.view.written).not.toContain("LATE");
  });

  it("calls unlisten exactly once when dispose() happens during an in-flight listen() that resolves late (StrictMode race)", async () => {
    const h = makeHarness({ deferListen: true });
    const startPromise = h.session.start();
    // Let start() build the view and reach the awaited listen() (still pending).
    await Promise.resolve();
    await Promise.resolve();
    expect(h.unlisten).not.toHaveBeenCalled();

    // Unmount while listen() is unresolved.
    h.session.dispose();
    // listen() now resolves late, AFTER dispose.
    h.resolveListen();
    await startPromise;

    // start() must have unlistened the late subscription exactly once, and
    // dispose() must not have double-unlistened (it never saw a stored unlisten).
    expect(h.unlisten).toHaveBeenCalledTimes(1);
    // The view created before dispose is torn down exactly once.
    expect(h.view.disposeCount).toBe(1);
  });

  it("start() is a no-op on an already-disposed session", async () => {
    const h = makeHarness({ snapshotImmediate: "SNAP" });
    h.session.dispose();
    await h.session.start();
    // No view built, no subscription, no snapshot written.
    expect(h.view.disposeCount).toBe(0);
    expect(h.listenCalledWith()).toBeUndefined();
    expect(h.view.written).toEqual([]);
  });
});

describe("TerminalSession resize debounce", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("reports the actual viewer geometry once at startup (immediate, not debounced)", async () => {
    const h = makeHarness({ snapshotImmediate: "" });
    await h.session.start();
    // start() fits and resizes once so the pty is not stuck at its initial size.
    expect(h.view.fitCount).toBe(1);
    expect(h.invoke).toHaveBeenCalledWith("agent_pty_resize", {
      agentId: "coder-1",
      cols: 80,
      rows: 24,
    });
  });

  it("coalesces a burst of resize requests into a single fit + agent_pty_resize", async () => {
    const h = makeHarness({ snapshotImmediate: "" });
    await h.session.start();
    // Baseline AFTER the one-shot startup resize.
    const baseFit = h.view.fitCount;
    h.invoke.mockClear();

    h.session.requestResize();
    h.session.requestResize();
    h.session.requestResize();
    // Nothing new fired yet (still inside the debounce window).
    expect(h.view.fitCount).toBe(baseFit);
    expect(h.invoke).not.toHaveBeenCalled();

    vi.advanceTimersByTime(150);
    expect(h.view.fitCount).toBe(baseFit + 1);
    expect(h.invoke).toHaveBeenCalledTimes(1);
    expect(h.invoke).toHaveBeenCalledWith("agent_pty_resize", {
      agentId: "coder-1",
      cols: 80,
      rows: 24,
    });
  });

  it("does NOT send agent_pty_resize when fit() failed (zero-size host)", async () => {
    const h = makeHarness({ snapshotImmediate: "" });
    // fit() fails for the WHOLE session, including the one-shot startup resize.
    h.view.fitOk.value = false;
    await h.session.start();
    expect(h.view.fitCount).toBeGreaterThan(0); // fit was attempted
    expect(h.invoke).not.toHaveBeenCalledWith(
      "agent_pty_resize",
      expect.anything(),
    );

    h.invoke.mockClear();
    h.session.requestResize();
    vi.advanceTimersByTime(150);
    expect(h.view.fitCount).toBeGreaterThan(0);
    expect(h.invoke).not.toHaveBeenCalledWith(
      "agent_pty_resize",
      expect.anything(),
    );
  });

  it("does NOT send agent_pty_resize when the grid reports a degenerate size", async () => {
    const h = makeHarness({ snapshotImmediate: "" });
    h.view.geometry.cols = 0;
    h.view.geometry.rows = 0;
    await h.session.start();
    expect(h.invoke).not.toHaveBeenCalledWith(
      "agent_pty_resize",
      expect.anything(),
    );
  });

  it("DOES send agent_pty_resize when fit() succeeds with a valid size", async () => {
    const h = makeHarness({ snapshotImmediate: "" });
    h.view.fitOk.value = true;
    await h.session.start();
    expect(h.invoke).toHaveBeenCalledWith("agent_pty_resize", {
      agentId: "coder-1",
      cols: 80,
      rows: 24,
    });
  });
});

describe("TerminalSession ctrl-c two-step", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("arms on the first request and sends ETX only on the second", async () => {
    const h = makeHarness({ snapshotImmediate: "" });
    await h.session.start();
    h.invoke.mockClear();

    h.session.requestCtrlC();
    expect(h.ctrlCStates[h.ctrlCStates.length - 1]).toBe(true);
    // No ETX written on arm.
    expect(h.invoke).not.toHaveBeenCalledWith(
      "agent_pty_write",
      expect.objectContaining({ data: "\x03" }),
    );

    h.session.requestCtrlC();
    expect(h.invoke).toHaveBeenCalledWith("agent_pty_write", {
      agentId: "coder-1",
      data: "\x03",
    });
    // Disarmed after sending.
    expect(h.ctrlCStates[h.ctrlCStates.length - 1]).toBe(false);
  });

  it("auto-disarms after the timeout without sending ETX", async () => {
    const h = makeHarness({ snapshotImmediate: "" });
    await h.session.start();
    h.invoke.mockClear();

    h.session.requestCtrlC();
    expect(h.ctrlCStates[h.ctrlCStates.length - 1]).toBe(true);

    vi.advanceTimersByTime(3000);
    expect(h.ctrlCStates[h.ctrlCStates.length - 1]).toBe(false);
    expect(h.invoke).not.toHaveBeenCalledWith(
      "agent_pty_write",
      expect.objectContaining({ data: "\x03" }),
    );
  });
});

describe("TerminalSession write failure banner", () => {
  it("surfaces an error banner only after repeated write failures", async () => {
    const h = makeHarness({ snapshotImmediate: "" });
    await h.session.start();
    // Make writes fail.
    h.invoke.mockImplementation(async (command: string) => {
      if (command === "agent_pty_write") throw new Error("dead pipe");
      return undefined;
    });

    await h.session.writeToPty("a");
    // First failure: no banner yet.
    expect(
      h.banners.some(
        (b) =>
          b?.kind === "error" &&
          b.message === "Could not send input to the agent terminal.",
      ),
    ).toBe(false);

    await h.session.writeToPty("b");
    // Second failure crosses the threshold.
    expect(
      h.banners.some(
        (b) =>
          b?.kind === "error" &&
          b.message === "Could not send input to the agent terminal.",
      ),
    ).toBe(true);
  });
});
