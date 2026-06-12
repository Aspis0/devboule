import { describe, expect, it, vi } from "vitest";
import type { ProjectGitStatus } from "../../types/backend";
import {
  censorChipAria,
  censorChipLabel,
  CensorCountsTracker,
  censorTrackedSignature,
  gitChipModel,
} from "./censorCounts";

// ---- pure censor chip helpers ----------------------------------------------

describe("censorChipLabel", () => {
  it("returns null for 0 (chip hidden when clean)", () => {
    expect(censorChipLabel(0)).toBeNull();
  });

  it("returns null for undefined (count not yet known)", () => {
    expect(censorChipLabel(undefined)).toBeNull();
  });

  it("returns null for negative / NaN (never trust a bad count)", () => {
    expect(censorChipLabel(-3)).toBeNull();
    expect(censorChipLabel(Number.NaN)).toBeNull();
  });

  it("formats a positive count with the warning glyph", () => {
    expect(censorChipLabel(1)).toBe("⚠1");
    expect(censorChipLabel(42)).toBe("⚠42");
  });

  it("floors a fractional count rather than rendering a decimal", () => {
    expect(censorChipLabel(2.9)).toBe("⚠2");
  });

  it("aria-label is null when hidden, descriptive when shown", () => {
    expect(censorChipAria(0)).toBeNull();
    expect(censorChipAria(undefined)).toBeNull();
    expect(censorChipAria(1)).toBe("1 open Censor finding");
    expect(censorChipAria(3)).toBe("3 open Censor findings");
  });
});

// ---- pure git chip model ----------------------------------------------------

function git(partial: Partial<ProjectGitStatus>): ProjectGitStatus {
  return {
    rootPath: null,
    repoRoot: null,
    repoName: null,
    branch: null,
    upstream: null,
    origin: null,
    githubUrl: null,
    cloneCommand: null,
    pullRequestUrl: null,
    commit: null,
    dirtyCount: 0,
    stagedCount: 0,
    unstagedCount: 0,
    untrackedCount: 0,
    aheadCount: 0,
    behindCount: 0,
    isGitRepo: true,
    isGithub: false,
    policyStatus: "ready",
    warnings: [],
    requiredActions: [],
    suggestedRepos: [],
    ...partial,
  };
}

describe("gitChipModel", () => {
  it("returns null when not a git repo", () => {
    expect(gitChipModel(git({ isGitRepo: false, dirtyCount: 5 }))).toBeNull();
  });

  it("returns null for a clean, in-sync repo (all-zero)", () => {
    expect(gitChipModel(git({}))).toBeNull();
  });

  it("returns null for null/undefined gitStatus", () => {
    expect(gitChipModel(undefined)).toBeNull();
    expect(gitChipModel(null)).toBeNull();
  });

  it("shows ahead when ahead > 0", () => {
    const model = gitChipModel(git({ aheadCount: 2 }));
    expect(model?.segments).toEqual(["↑2"]);
    expect(model?.ariaLabel).toBe("Git: 2 ahead");
  });

  it("shows behind when behind > 0", () => {
    const model = gitChipModel(git({ behindCount: 3 }));
    expect(model?.segments).toEqual(["↓3"]);
    expect(model?.ariaLabel).toBe("Git: 3 behind");
  });

  it("shows the dirty count (∆) when there are uncommitted changes", () => {
    const model = gitChipModel(git({ dirtyCount: 4 }));
    expect(model?.segments).toEqual(["4∆"]);
    expect(model?.ariaLabel).toBe("Git: 4 uncommitted changes");
  });

  it("combines ahead, behind and dirty in a stable order", () => {
    const model = gitChipModel(
      git({ aheadCount: 1, behindCount: 2, dirtyCount: 5 }),
    );
    expect(model?.segments).toEqual(["↑1", "↓2", "5∆"]);
    expect(model?.ariaLabel).toBe(
      "Git: 1 ahead, 2 behind, 5 uncommitted changes",
    );
  });

  it("ignores negative/NaN counters defensively", () => {
    expect(gitChipModel(git({ aheadCount: -1, dirtyCount: Number.NaN }))).toBeNull();
  });
});

// ---- tracked-set signature (re-bind key) -----------------------------------

describe("censorTrackedSignature", () => {
  it("is order-independent for the same set (no needless re-bind)", () => {
    const a = censorTrackedSignature([
      { id: "p1", rootPath: "C:/a" },
      { id: "p2", rootPath: "C:/b" },
    ]);
    const b = censorTrackedSignature([
      { id: "p2", rootPath: "C:/b" },
      { id: "p1", rootPath: "C:/a" },
    ]);
    expect(a).toBe(b);
  });

  it("differs when a project is added or removed", () => {
    const one = censorTrackedSignature([{ id: "p1", rootPath: "C:/a" }]);
    const two = censorTrackedSignature([
      { id: "p1", rootPath: "C:/a" },
      { id: "p2", rootPath: "C:/b" },
    ]);
    expect(one).not.toBe(two);
  });

  it("differs when a root changes", () => {
    const before = censorTrackedSignature([{ id: "p1", rootPath: "C:/a" }]);
    const after = censorTrackedSignature([{ id: "p1", rootPath: "C:/z" }]);
    expect(before).not.toBe(after);
  });

  it("does NOT collide across different sets that a separator-less join would alias", () => {
    // The bug: joining "id root" with "" lets {id:"ab", root:""} and
    // {id:"a", root:"b"} (or shifting a char across the id/root boundary)
    // produce the same string. With proper delimiters they must differ.
    const setA = censorTrackedSignature([{ id: "ab", rootPath: "" }]);
    const setB = censorTrackedSignature([{ id: "a", rootPath: "b" }]);
    expect(setA).not.toBe(setB);

    // And shifting a boundary between two entries must also be distinguishable.
    const setC = censorTrackedSignature([
      { id: "p", rootPath: "1" },
      { id: "q", rootPath: "2" },
    ]);
    const setD = censorTrackedSignature([
      { id: "p", rootPath: "1q" },
      { id: "", rootPath: "2" },
    ]);
    expect(setC).not.toBe(setD);
  });
});

// ---- event-driven count tracker (no poller) --------------------------------

interface Project {
  id: string;
  rootPath: string | null;
}

const CHANNEL = "censor://findings-updated";

// Robust macrotask drain: lets the tracker's chained Promise.all + finally +
// onChange publish (and any coalesced trailing refetch it kicks) settle before
// we assert. A setTimeout(0) macrotask flushes the ENTIRE microtask queue
// regardless of how deep the await chain is, so adding a project to a fixture
// can never silently under-drain (the failure mode of a fixed-N microtask loop).
// Two turns cover a sweep that itself kicks one coalesced follow-up sweep.
async function flush(): Promise<void> {
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
}

// A controllable fake `listen`. CRUCIAL: it collects EVERY registered handler
// for a channel in an array (not a single overwriting slot), so a test can
// detect an orphaned second subscription. `fire` invokes ALL live handlers for a
// channel; `activeHandlers` reports how many are currently registered. Each
// `listen` call returns an unlisten that removes only ITS handler, mirroring
// Tauri's per-subscription unlisten.
function makeFakeListen() {
  const handlers: Record<string, Array<(payload: unknown) => void>> = {};
  const unlisten = vi.fn();
  const listen = vi.fn(
    async (channel: string, handler: (e: { payload: unknown }) => void) => {
      const wrapped = (payload: unknown) => handler({ payload });
      (handlers[channel] ??= []).push(wrapped);
      return () => {
        unlisten();
        const list = handlers[channel];
        if (list) {
          const index = list.indexOf(wrapped);
          if (index >= 0) list.splice(index, 1);
        }
      };
    },
  );
  const fire = (channel: string, payload: unknown) => {
    for (const handler of [...(handlers[channel] ?? [])]) handler(payload);
  };
  const activeHandlers = (channel: string) => handlers[channel]?.length ?? 0;
  return { handlers, unlisten, listen, fire, activeHandlers };
}

describe("CensorCountsTracker", () => {
  it("fetches one count per project with a root and exposes the map", async () => {
    const projects: Project[] = [
      { id: "p1", rootPath: "C:/a" },
      { id: "p2", rootPath: "C:/b" },
    ];
    const invoke = vi.fn(async (_cmd: string, args?: Record<string, unknown>) =>
      args?.root === "C:/a" ? 3 : 7,
    );
    const { listen } = makeFakeListen();
    const updates: Array<Record<string, number>> = [];

    const tracker = new CensorCountsTracker({
      invoke,
      listen,
      onChange: (m) => updates.push(m),
    });
    await tracker.start(projects);

    expect(invoke).toHaveBeenCalledTimes(2);
    expect(invoke).toHaveBeenCalledWith("censor_count_open", { root: "C:/a" });
    expect(updates[updates.length - 1]).toEqual({ p1: 3, p2: 7 });
    tracker.stop();
  });

  it("treats a project with no root as 0 and never invokes for it", async () => {
    const projects: Project[] = [
      { id: "p1", rootPath: null },
      { id: "p2", rootPath: "   " },
    ];
    const invoke = vi.fn(async () => 5);
    const { listen } = makeFakeListen();

    const tracker = new CensorCountsTracker({ invoke, listen });
    await tracker.start(projects);

    expect(invoke).not.toHaveBeenCalled();
    expect(tracker.counts).toEqual({ p1: 0, p2: 0 });
    tracker.stop();
  });

  it("does not publish a first all-zero map (no chips to render)", async () => {
    const projects: Project[] = [{ id: "p1", rootPath: null }];
    const invoke = vi.fn(async () => 0);
    const { listen } = makeFakeListen();
    const updates: Array<Record<string, number>> = [];

    const tracker = new CensorCountsTracker({
      invoke,
      listen,
      onChange: (m) => updates.push(m),
    });
    await tracker.start(projects);

    // The map is tracked internally (so a later non-zero count diffs against it)
    // but the first all-zero state never fires a no-op board render.
    expect(tracker.counts).toEqual({ p1: 0 });
    expect(updates).toEqual([]);
    tracker.stop();
  });

  it("a failed count degrades to 0 and never throws", async () => {
    const projects: Project[] = [{ id: "p1", rootPath: "C:/a" }];
    const invoke = vi.fn(async () => {
      throw new Error("backend exploded");
    });
    const { listen } = makeFakeListen();

    const tracker = new CensorCountsTracker({ invoke, listen });
    await expect(tracker.start(projects)).resolves.toBeUndefined();
    expect(tracker.counts).toEqual({ p1: 0 });
    tracker.stop();
  });

  it("refetches on a censor://findings-updated event (no poller)", async () => {
    const projects: Project[] = [{ id: "p1", rootPath: "C:/a" }];
    let next = 1;
    const invoke = vi.fn(async () => next);
    const { fire, listen } = makeFakeListen();
    const updates: Array<Record<string, number>> = [];

    const tracker = new CensorCountsTracker({
      invoke,
      listen,
      onChange: (m) => updates.push(m),
    });
    await tracker.start(projects);
    expect(tracker.counts).toEqual({ p1: 1 });

    // A new finding arrives -> the backend now reports 2.
    next = 2;
    fire(CHANNEL, { projectId: "p1", files: ["a.ts"] });
    await flush();
    expect(invoke).toHaveBeenCalledTimes(2);
    expect(tracker.counts).toEqual({ p1: 2 });
    expect(updates[updates.length - 1]).toEqual({ p1: 2 });
    tracker.stop();
  });

  it("a known projectId refetches ONLY that project (one invoke, not N)", async () => {
    const projects: Project[] = [
      { id: "p1", rootPath: "C:/a" },
      { id: "p2", rootPath: "C:/b" },
      { id: "p3", rootPath: "C:/c" },
    ];
    const counts: Record<string, number> = { "C:/a": 1, "C:/b": 1, "C:/c": 1 };
    const invoke = vi.fn(
      async (_cmd: string, args?: Record<string, unknown>) =>
        counts[String(args?.root)] ?? 0,
    );
    const { fire, listen } = makeFakeListen();
    const updates: Array<Record<string, number>> = [];

    const tracker = new CensorCountsTracker({
      invoke,
      listen,
      onChange: (m) => updates.push(m),
    });
    await tracker.start(projects);
    expect(invoke).toHaveBeenCalledTimes(3); // initial full sweep
    invoke.mockClear();

    // p2 gains a finding; the event names p2 -> targeted single refetch.
    counts["C:/b"] = 5;
    fire(CHANNEL, { projectId: "p2", files: ["b.ts"] });
    await flush();

    expect(invoke).toHaveBeenCalledTimes(1); // NOT 3
    expect(invoke).toHaveBeenCalledWith("censor_count_open", { root: "C:/b" });
    expect(tracker.counts).toEqual({ p1: 1, p2: 5, p3: 1 });
    tracker.stop();
  });

  it("an unknown / missing projectId falls back to a full sweep", async () => {
    const projects: Project[] = [
      { id: "p1", rootPath: "C:/a" },
      { id: "p2", rootPath: "C:/b" },
    ];
    let value = 1;
    const invoke = vi.fn(async () => value);
    const { fire, listen } = makeFakeListen();

    const tracker = new CensorCountsTracker({ invoke, listen });
    await tracker.start(projects);
    expect(invoke).toHaveBeenCalledTimes(2);
    invoke.mockClear();

    // Unknown projectId -> full sweep (every tracked project re-fetched).
    value = 4;
    fire(CHANNEL, { projectId: "not-tracked", files: [] });
    await flush();
    expect(invoke).toHaveBeenCalledTimes(2);
    expect(tracker.counts).toEqual({ p1: 4, p2: 4 });

    // No payload at all -> also a full sweep.
    invoke.mockClear();
    value = 6;
    fire(CHANNEL, undefined);
    await flush();
    expect(invoke).toHaveBeenCalledTimes(2);
    expect(tracker.counts).toEqual({ p1: 6, p2: 6 });
    tracker.stop();
  });

  it("coalesces a burst of events into at most one trailing refetch", async () => {
    const projects: Project[] = [{ id: "p1", rootPath: "C:/a" }];
    // Gate that holds a sweep open. Disarmed during start() so the initial sweep
    // resolves; armed afterwards so the event-driven sweep stays in flight while
    // the rest of the burst arrives.
    let gated = false;
    const gateHolder: { release: () => void } = { release: () => {} };
    const gate = () =>
      new Promise<void>((resolve) => {
        gateHolder.release = resolve;
      });
    const invoke = vi.fn(async () => {
      if (gated) await gate();
      return 9;
    });
    const { fire, listen } = makeFakeListen();

    const tracker = new CensorCountsTracker({ invoke, listen });
    await tracker.start(projects); // initial sweep (ungated) settles here
    invoke.mockClear();
    gated = true;

    // Five events while a sweep is in flight collapse to ONE trailing refetch
    // (the first event kicks a sweep; the other four only raise the coalesce
    // flag), so at most two invokes total — never five.
    for (let i = 0; i < 5; i += 1) {
      fire(CHANNEL, { projectId: "p1", files: [] });
    }
    await Promise.resolve(); // let the first sweep reach its gated await
    gated = false;
    gateHolder.release(); // release the in-flight sweep; the trailing one is ungated
    await flush();
    expect(invoke.mock.calls.length).toBeLessThanOrEqual(2);
    expect(invoke.mock.calls.length).toBeGreaterThanOrEqual(1);
    tracker.stop();
  });

  it("does not re-publish (no board re-render) when an event leaves counts unchanged", async () => {
    const projects: Project[] = [{ id: "p1", rootPath: "C:/a" }];
    const invoke = vi.fn(async () => 5); // count never changes
    const { fire, listen } = makeFakeListen();
    const updates: Array<Record<string, number>> = [];

    const tracker = new CensorCountsTracker({
      invoke,
      listen,
      onChange: (m) => updates.push(m),
    });
    await tracker.start(projects);
    expect(updates.length).toBe(1); // initial publish only

    // An event for an unrelated file: the open count is still 5, so onChange must
    // NOT fire again (the equality guard suppresses the no-op re-render).
    fire(CHANNEL, { projectId: "p1", files: ["x.ts"] });
    await flush();
    expect(invoke.mock.calls.length).toBeGreaterThanOrEqual(2); // it DID refetch
    expect(updates.length).toBe(1); // but published nothing new
    tracker.stop();
  });

  it("stop() unsubscribes the listener (no leak), leaves zero active handlers, and ignores late events", async () => {
    const projects: Project[] = [{ id: "p1", rootPath: "C:/a" }];
    const invoke = vi.fn(async () => 4);
    const { fire, unlisten, activeHandlers, listen } = makeFakeListen();

    const tracker = new CensorCountsTracker({ invoke, listen });
    await tracker.start(projects);
    expect(activeHandlers(CHANNEL)).toBe(1);
    invoke.mockClear();

    tracker.stop();
    expect(unlisten).toHaveBeenCalledTimes(1);
    expect(activeHandlers(CHANNEL)).toBe(0); // no orphaned listener

    // An event after stop() must not trigger any further work.
    fire(CHANNEL, { projectId: "p1", files: [] });
    await flush();
    expect(invoke).not.toHaveBeenCalled();
  });

  it("start() after stop() drops the prior listener — exactly ONE active handler, never two", async () => {
    const invoke = vi.fn(async () => 0);
    const { unlisten, activeHandlers, listen } = makeFakeListen();
    const tracker = new CensorCountsTracker({ invoke, listen });

    await tracker.start([{ id: "p1", rootPath: "C:/a" }]);
    expect(activeHandlers(CHANNEL)).toBe(1);

    await tracker.start([{ id: "p2", rootPath: "C:/b" }]);

    // The first subscription must have been torn down so we never double-listen:
    // exactly one live handler, and unlisten called once for the prior one.
    expect(activeHandlers(CHANNEL)).toBe(1);
    expect(unlisten).toHaveBeenCalledTimes(1);
    expect(listen).toHaveBeenCalledTimes(2);
    tracker.stop();
  });

  it("start() called mid-sweep (start-over-start) never runs a concurrent/extra sweep (M-5)", async () => {
    const projects: Project[] = [{ id: "p1", rootPath: "C:/a" }];
    // The exact start-over-start race: a listener-driven sweep is in flight when
    // start() is called again. Generation 1's event-driven sweep is parked on an
    // invoke; the new start()'s teardown supersedes it. When the stale sweep
    // later resolves, its finally must NOT touch the NEW generation's `sweeping`
    // flag (or drain its pending) — otherwise a subsequent event fires a sweep
    // concurrent with the new generation's own sweep. Each invoke parks on its
    // own resolver so sweeps can overlap and be released independently.
    const releasers: Array<() => void> = [];
    let gated = false;
    const invoke = vi.fn(() => {
      if (!gated) return Promise.resolve(1);
      return new Promise<number>((resolve) => {
        releasers.push(() => resolve(1));
      });
    });
    const { fire, listen } = makeFakeListen();

    const tracker = new CensorCountsTracker({ invoke, listen });

    // First start completes fully and SUBSCRIBES the listener (ungated sweep).
    await tracker.start(projects);
    expect(invoke).toHaveBeenCalledTimes(1);

    // An event kicks generation 1's event-driven sweep; gate it so it stays in
    // flight (parked on invoke #1 / releasers[0]).
    gated = true;
    fire(CHANNEL, { projectId: "p1", files: [] });
    await flush();
    expect(invoke).toHaveBeenCalledTimes(2); // gen-1 event sweep parked

    // start() AGAIN while that sweep is in flight. teardown() supersedes gen 1
    // and resets `sweeping`; the new generation's own initial sweep runs ungated.
    gated = false;
    await tracker.start(projects);
    const invokesAfterRestart = invoke.mock.calls.length; // gen-2 initial sweep ran

    // Release the STALE gen-1 event sweep. It sees the epoch mismatch and drops
    // its result; its epoch-guarded finally must be a complete no-op (no flag
    // flip, no drain). releasers[0] is that parked invoke.
    releasers[0]?.();
    await flush();

    // The new generation must be in a clean, idle state: a fresh event triggers
    // EXACTLY ONE more invoke (1 project = 1 targeted refetch). If teardown had
    // left `sweeping` true, this event would be coalesced forever (+0); if the
    // stale finally had raced, it would collide with a phantom sweep (>+1).
    fire(CHANNEL, { projectId: "p1", files: ["z.ts"] });
    await flush();
    expect(invoke.mock.calls.length).toBe(invokesAfterRestart + 1);
    tracker.stop();
  });
});
