// Focused unit tests for the Polis live AGENT poll guard logic in cityStore.
//
// These pin the L3 "honest + robust telemetry" invariants flagged by the F0
// audit. The guard logic lives in module-level closures (start/stop poll), so we
// drive it through the real store with:
//   - the AppContext module mocked (isTauriRuntime -> true; invokeBackendCommand
//     -> a controllable deferred so we can hold a refresh "in flight" and inspect
//     concurrency precisely);
//   - fake timers (the poll re-arms via window.setTimeout every AGENT_POLL_MS);
//   - a fake document.visibilityState ("visible") so ticks actually fire.
//
// Invariants verified:
//   (a) at most ONE polis_refresh_agents is in flight at a time — even across a
//       stop()/start() restart while a prior request is still on the wire;
//   (b) a stop() bumps the epoch so an in-flight result is DROPPED on resolve
//       (never applied to the city after the poll was stopped);
//   (c) a restart after stop() does NOT strand the in-flight flag (a later tick
//       fires a fresh request once the prior one settles);
//   (d) a result is applied ONLY if the watched folder still matches (a folder
//       switch mid-flight drops the stale result — no agents for a stale city).

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import type { CityState } from "../types/city";

// ---- Controllable backend mock -------------------------------------------
//
// Each invokeBackendCommand("polis_refresh_agents") call pushes a deferred we
// resolve manually, so a request can be held "in flight" for as long as a test
// needs to observe concurrency.
interface Deferred {
  command: string;
  resolve: (city: CityState) => void;
  reject: (err: unknown) => void;
}
const pending: Deferred[] = [];
let invokeCalls = 0;

vi.mock("../context/AppContext", () => ({
  isTauriRuntime: () => true,
  invokeBackendCommand: (command: string) => {
    // Temporary Phase-0 diagnostic channel (logCityComposition): fire-and-forget
    // log lines, irrelevant to the poll-concurrency invariants pinned here.
    // Swallow them so they don't perturb invokeCalls/pending. Remove this branch
    // together with the instrumentation.
    if (command === "polis_debug_log") {
      return Promise.resolve(undefined as unknown as CityState);
    }
    invokeCalls += 1;
    return new Promise<CityState>((resolve, reject) => {
      pending.push({ command, resolve, reject });
    });
  },
}));

// A minimal-but-real CityState with one building so the store treats it as a
// loaded city the poll is willing to refresh.
function mkCity(label: string): CityState {
  return {
    projectName: label,
    era: "Alpha",
    generatedAt: "",
    buildings: [
      {
        fileId: "fid-1",
        filePath: "src/a.ts",
        districtId: "core",
        purpose: "house",
        purposeSource: "default",
        featureId: "commons",
        featureSource: "commons",
        provider: null,
        linesOfCode: 10,
        visualTier: "kalybe",
        coords: { x: 0, y: 0 },
        status: "normal",
        label: "a.ts",
        description: "",
        lastModified: "",
        agentPresent: null,
        kanbanCardId: null,
        untrackedChange: null,
        sins: [],
        notes: [],
      },
    ],
    roads: [],
    districts: [],
    agents: [],
    sins: [],
    externalServices: [],
    features: [],
    notes: [],
  } as unknown as CityState;
}

// Resolve the single oldest pending refresh with a city, then flush the task
// queues so the awaiting tick body (apply + finally) runs to completion.
async function resolvePendingWith(city: CityState): Promise<void> {
  const d = pending.shift();
  if (!d) throw new Error("no pending refresh to resolve");
  d.resolve(city);
  await flush();
}

// FIX 2(a): drain BOTH the micro- and macro-task queues deterministically.
// Two `await Promise.resolve()` only drained microtasks and was fragile vs
// Zustand's notification scheduling; a `setTimeout(0)` under fake timers (driven
// by advanceTimersByTimeAsync) drains the awaited promise chain AND any queued
// macrotask so the tick's apply + finally have definitely run on return.
async function flush(): Promise<void> {
  await vi.advanceTimersByTimeAsync(0);
}

let useCityStore: typeof import("./cityStore").useCityStore;
let inFlight: typeof import("./cityStore").__agentPollInFlightForTest;

beforeEach(async () => {
  vi.resetModules();
  pending.length = 0;
  invokeCalls = 0;

  // Fake timers so we control the poll's setTimeout re-arm deterministically.
  vi.useFakeTimers();

  // jsdom is not the env (node), so provide the globals the store touches.
  // localStorage: the store reads it at module-init for the restored folder.
  const store = new Map<string, string>();
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (globalThis as any).window = globalThis;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (globalThis as any).localStorage = {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => void store.set(k, v),
    removeItem: (k: string) => void store.delete(k),
  };
  // document.visibilityState must be "visible" for ticks to fire.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (globalThis as any).document = { visibilityState: "visible" };

  // Import the store AFTER the mock + globals are installed.
  ({ useCityStore, __agentPollInFlightForTest: inFlight } = await import(
    "./cityStore"
  ));
});

afterEach(() => {
  // Stop any poll so a stray timer can't leak across tests.
  useCityStore.getState().stopAgentPoll();
  vi.clearAllTimers();
  vi.useRealTimers();
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  delete (globalThis as any).document;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  delete (globalThis as any).localStorage;
});

const AGENT_POLL_MS = 5_000;

describe("agent poll guards", () => {
  it("(a)+(c) at most one refresh in flight across a stop/restart, no stranded flag", async () => {
    const s = useCityStore.getState();
    // A loaded city + folder so the poll is willing to refresh.
    useCityStore.setState({ cityState: mkCity("p"), selectedFolder: "C:/p" });

    s.startAgentPoll();
    // First tick fires after one interval -> one request on the wire.
    await vi.advanceTimersByTimeAsync(AGENT_POLL_MS);
    expect(pending.length).toBe(1);
    expect(invokeCalls).toBe(1);
    // DIRECT: the global in-flight flag is true while the request is on the wire.
    expect(inFlight()).toBe(true);

    // Restart WHILE the first request is still in flight. The old bug cleared
    // the in-flight flag on stop, letting the restart fire a 2nd concurrent
    // request. With the fix, stop does not clear it, so even after the restart's
    // first tick elapses, NO second request goes out while the first is pending.
    s.stopAgentPoll();
    // DIRECT: stop must NOT clear the flag (only the issuing tick's finally does).
    expect(inFlight()).toBe(true);
    s.startAgentPoll();
    await vi.advanceTimersByTimeAsync(AGENT_POLL_MS);
    expect(pending.length).toBe(1); // still exactly one in flight
    expect(invokeCalls).toBe(1);
    expect(inFlight()).toBe(true);

    // Resolve the in-flight request: its finally clears the flag. Its result is
    // dropped (epoch bumped by stop) — see the (b) test for that assertion.
    await resolvePendingWith(mkCity("p"));
    expect(pending.length).toBe(0);
    // DIRECT (c): the flag is cleared on settle — never stranded true after a
    // stop/restart while a request was on the wire.
    expect(inFlight()).toBe(false);

    // Now the restarted poll is free to fire again on its next tick (not stranded).
    await vi.advanceTimersByTimeAsync(AGENT_POLL_MS);
    expect(pending.length).toBe(1);
    expect(invokeCalls).toBe(2);
  });

  it("(b) an in-flight result is dropped after stop (epoch mismatch)", async () => {
    const s = useCityStore.getState();
    const original = mkCity("p");
    useCityStore.setState({ cityState: original, selectedFolder: "C:/p" });

    s.startAgentPoll();
    await vi.advanceTimersByTimeAsync(AGENT_POLL_MS);
    expect(pending.length).toBe(1);

    // Stop the poll, THEN let the in-flight request resolve with a different city.
    s.stopAgentPoll();
    const stale = mkCity("STALE");
    await resolvePendingWith(stale);

    // The stale result must NOT have been applied (poll was stopped -> dropped).
    expect(useCityStore.getState().cityState).toBe(original);
    expect(useCityStore.getState().cityState).not.toBe(stale);
  });

  it("(d) a result for a folder that changed mid-flight is dropped", async () => {
    const s = useCityStore.getState();
    const cityA = mkCity("A");
    useCityStore.setState({ cityState: cityA, selectedFolder: "C:/A" });

    s.startAgentPoll();
    await vi.advanceTimersByTimeAsync(AGENT_POLL_MS);
    expect(pending.length).toBe(1);

    // The user switches folders while the refresh is in flight. (We set the
    // folder + a fresh city directly to simulate a completed loadFolder.)
    const cityB = mkCity("B");
    useCityStore.setState({ cityState: cityB, selectedFolder: "C:/B" });

    // The in-flight request (issued for folder A) resolves with A's agents.
    await resolvePendingWith(mkCity("A-agents"));

    // It must be DROPPED: the live city must still be B's, not the A result.
    expect(useCityStore.getState().cityState).toBe(cityB);
    expect(useCityStore.getState().selectedFolder).toBe("C:/B");
  });

  it("(FIX 1) a SAME-folder reload mid-flight drops the stale agent result (no clobber)", async () => {
    const s = useCityStore.getState();
    const cityA = mkCity("A");
    useCityStore.setState({ cityState: cityA, selectedFolder: "C:/p" });

    s.startAgentPoll();
    await vi.advanceTimersByTimeAsync(AGENT_POLL_MS);
    expect(pending.length).toBe(1); // agent refresh in flight, captured requestSeq

    // A SAME-folder reload completes while the agent refresh is still on the wire:
    // loadFolder("C:/p") -> ++requestSeq, then writes the freshly-scanned city.
    // The folder is UNCHANGED, so the folder gate alone would let the stale agent
    // result through; the requestSeq gate is what must drop it. We drive the real
    // loadFolder so the seq bumps exactly as in production, then resolve its scan
    // (the next pending invoke) with the fresh city.
    const reloaded = mkCity("RELOADED");
    void s.loadFolder("C:/p");
    // loadFolder enqueued a generate_city_state invoke; resolve it -> new city set.
    expect(pending.length).toBe(2);
    // Resolve the reload's scan (match by command, not position) with the fresh city.
    const idx = pending.findIndex((d) => d.command === "generate_city_state");
    expect(idx).toBeGreaterThanOrEqual(0);
    const reloadDeferred = pending.splice(idx, 1)[0];
    reloadDeferred.resolve(reloaded);
    await flush();
    expect(useCityStore.getState().cityState).toBe(reloaded);

    // NOW the stale agent refresh (issued before the reload) resolves.
    await resolvePendingWith(mkCity("STALE-agents"));

    // It must be DROPPED: requestSeq advanced past seqAtRequest -> the fresh
    // reloaded city is NOT clobbered by the stale agent result.
    expect(useCityStore.getState().cityState).toBe(reloaded);
  });

  it("applies a fresh result when epoch + folder still match", async () => {
    const s = useCityStore.getState();
    useCityStore.setState({ cityState: mkCity("p"), selectedFolder: "C:/p" });

    s.startAgentPoll();
    await vi.advanceTimersByTimeAsync(AGENT_POLL_MS);
    expect(pending.length).toBe(1);

    const refreshed = mkCity("refreshed");
    await resolvePendingWith(refreshed);

    // Applied via applyLiveUpdate: cityState becomes the refreshed city, and
    // liveCity carries it with a seq for the renderer diff.
    const cur = useCityStore.getState();
    expect(cur.cityState).toBe(refreshed);
    expect(cur.liveCity?.city).toBe(refreshed);
    expect(cur.usingFixture).toBe(false);
  });

  it("stopAgentPoll clears the re-arm timer (no leaked interval)", async () => {
    const s = useCityStore.getState();
    useCityStore.setState({ cityState: mkCity("p"), selectedFolder: "C:/p" });

    s.startAgentPoll();
    await vi.advanceTimersByTimeAsync(AGENT_POLL_MS);
    await resolvePendingWith(mkCity("p"));
    expect(invokeCalls).toBe(1);

    s.stopAgentPoll();
    // After stop, advancing time fires NO further requests (timer was cleared,
    // the stale tick does not re-arm).
    await vi.advanceTimersByTimeAsync(AGENT_POLL_MS * 5);
    expect(invokeCalls).toBe(1);
    expect(pending.length).toBe(0);
  });
});

// REGRESSION (Polis grey-map/OOM root cause): the fs-watcher and the agent poll
// kept re-delivering an IDENTICAL city every ~5s; each apply bumped liveSeq,
// which re-fired the view's diff effect and cancelled the in-flight chunked
// build — the map never settled. applyLiveUpdate must DROP a re-delivery whose
// only differences are the volatile fields (generatedAt, per-building
// lastModified) and still apply real content changes.
describe("applyLiveUpdate skip-if-unchanged", () => {
  it("drops an identical re-delivery (volatile fields excluded), applies real changes", () => {
    const s = useCityStore.getState();

    const first = mkCity("p");
    s.applyLiveUpdate(first);
    const afterFirst = useCityStore.getState();
    expect(afterFirst.cityState).toBe(first);
    const seqFirst = afterFirst.liveCity?.seq;
    expect(seqFirst).toBeDefined();

    // Degenerate case: the exact SAME object delivered again -> DROPPED.
    s.applyLiveUpdate(first);
    expect(useCityStore.getState().liveCity?.seq).toBe(seqFirst);

    // Same content, only the volatile fields differ (a rescan refreshes the
    // timestamp; a file save touches mtime) -> DROPPED: state object untouched,
    // seq NOT bumped (a bump would cancel/restart the renderer's build).
    const identical = mkCity("p");
    identical.generatedAt = "2026-06-10T12:00:00Z";
    identical.buildings[0].lastModified = "2026-06-10T12:00:00Z";
    s.applyLiveUpdate(identical);
    const afterIdentical = useCityStore.getState();
    expect(afterIdentical.cityState).toBe(first);
    expect(afterIdentical.liveCity?.seq).toBe(seqFirst);

    // A real content change -> applied, seq bumped exactly once.
    const changed = mkCity("p");
    changed.buildings[0].linesOfCode = 999;
    s.applyLiveUpdate(changed);
    const afterChanged = useCityStore.getState();
    expect(afterChanged.cityState).toBe(changed);
    expect(afterChanged.liveCity?.seq).toBe((seqFirst ?? 0) + 1);
  });
});
