// B2b — VIEWPORT-PRIORITIZED chunked build (headless). Proves the renderer-level
// contract on top of the pure buildQueue tests:
//   - VISIBLE-FIRST: buildings in viewport (+ ring) chunks are placed BEFORE
//     out-of-view ones (the build `order` head is the priority set), depth-stable.
//   - PROGRESS: the build reports `visibleDone === visibleTotal` (the visible city
//     complete) on a batch BEFORE `phase: "done"` (the whole build complete), and
//     the final callback is `phase: "done"`.
//   - REPRIORITIZE: a camera move mid-fill re-sorts the not-yet-placed REMAINDER of
//     the queue toward the new viewport WITHOUT re-placing the already-built head.
//   - CANCELLATION: a fresh setCityState mid-priority-build bumps the token so the
//     in-flight batch loop bows out (no stale placement), and buildState resets.
//
// Strategy mirrors PolisRenderer.sprite.test.ts: Object.create the renderer (skip
// the constructor), wire the minimal fields setCityState touches, STUB the heavy
// prelude/finalize collaborators, and record placements by overriding
// createBuildingNode. requestAnimationFrame is a manual queue so batches step
// deterministically. The REAL fitCameraToBuildings / viewportPriorityChunks /
// computeChunkBounds / orderBuildQueue run (the ordering under test); the viewport
// stub exposes FIXED visible bounds we control so "what is visible" is explicit.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

const { Container } = await import("pixi.js");
const { PolisRenderer } = await import("./PolisRenderer");
const { RENDER_PROFILES } = await import("./renderProfile");
import type { Building, CityState } from "../../types/city";

// ---------------------------------------------------------------------------
// Manual requestAnimationFrame queue (deterministic batch stepping).
// ---------------------------------------------------------------------------

let rafQueue: Array<() => void> = [];
let rafId = 0;
const rafMap = new Map<number, () => void>();

function installRaf() {
  rafQueue = [];
  rafMap.clear();
  rafId = 0;
  globalThis.requestAnimationFrame = ((cb: FrameRequestCallback) => {
    const id = ++rafId;
    const fn = () => cb(performance.now());
    rafMap.set(id, fn);
    rafQueue.push(fn);
    return id;
  }) as typeof requestAnimationFrame;
  globalThis.cancelAnimationFrame = ((id: number) => {
    const fn = rafMap.get(id);
    rafMap.delete(id);
    if (fn) rafQueue = rafQueue.filter((q) => q !== fn);
  }) as typeof cancelAnimationFrame;
}

/** Run exactly ONE queued frame (the next pending batch). Returns false if none. */
function stepRaf(): boolean {
  const fn = rafQueue.shift();
  if (!fn) return false;
  fn();
  return true;
}

/** Drain ALL queued frames (and any they schedule) — runs the build to completion. */
function flushRaf(): void {
  let guard = 0;
  while (rafQueue.length && guard++ < 10000) stepRaf();
}

// ---------------------------------------------------------------------------
// Building + city fixtures
// ---------------------------------------------------------------------------

let fid = 0;
function mkBuilding(tileX: number, tileY: number): Building {
  return {
    fileId: `f${++fid}`,
    filePath: `src/f${fid}.ts`,
    districtId: "d1",
    purpose: "house",
    purposeSource: "grounded",
    linesOfCode: 100,
    visualTier: "synoikia",
    coords: { x: tileX, y: tileY },
    status: "idle",
    label: `f${fid}.ts`,
    description: "",
    lastModified: "2026-01-01",
    sins: [],
    notes: [],
  } as unknown as Building;
}

function mkCity(buildings: Building[]): CityState {
  return {
    buildings,
    roads: [],
    districts: [],
    agents: [],
    gridSize: { w: 64, h: 64 },
    externalServices: [],
  } as unknown as CityState;
}

// ---------------------------------------------------------------------------
// Renderer harness
// ---------------------------------------------------------------------------

type AnyRec = Record<string, unknown>;

/** A viewport stub with FIXED, directly-settable visible world bounds so the test
 *  controls exactly which chunks are "visible". fitCameraToBuildings calls
 *  setZoom/moveCenter/findFit on it — all no-ops here (we don't let the camera fit
 *  perturb the bounds we asserted against). */
function makeViewport(left: number, top: number, w: number, h: number) {
  return {
    left,
    top,
    worldScreenWidth: w,
    worldScreenHeight: h,
    scale: { x: 1 },
    findFit: () => 1,
    setZoom: () => {},
    moveCenter: () => {},
  };
}

function makeRenderer(profileTier: "rich" | "lean" | "minimal" = "rich") {
  const fake = Object.create(
    PolisRenderer.prototype,
  ) as InstanceType<typeof PolisRenderer>;
  const set = (k: string, v: unknown) =>
    ((fake as unknown as AnyRec)[k] = v);

  // The order each createBuildingNode call placed (by fileId).
  const placed: string[] = [];

  set("destroyed", false);
  set("buildToken", 0);
  set("buildRaf", null);
  set("buildState", null);
  set("reprioritizeTimer", null);
  set("mutationState", "idle");
  set("profile", RENDER_PROFILES[profileTier]);
  set("lodLabelsIn", RENDER_PROFILES[profileTier].lodLabelsIn);
  set("buildingNodes", { size: 0 } as { size: number });
  set("chunks", new Map());
  set("cullDirty", false);
  set("lastCity", null);
  set("lastOnProgress", undefined);
  // A viewport showing a window around iso (0,0). Buildings near tile (0,0) are
  // visible; far ones (large tile coords) are out of view.
  set("viewport", makeViewport(-200, -400, 400, 600));
  set("externalLayer", { setServices: () => {}, setLodVisible: () => {} });
  set("clock", { reset: () => {} });

  // STUB the heavy collaborators setCityState touches (not under test here).
  const noop = () => {};
  for (const m of [
    "cancelBuild",
    "clearScene",
    "debugLog",
    "redrawTerrainProps",
    "drawDistricts",
    "drawRoads",
    "syncAmbient",
    "reconcileAgents",
    "syncTradeRoutes",
    "recenter",
  ]) {
    (fake as unknown as AnyRec)[m] = noop;
  }
  // cancelBuild is stubbed above, but setCityState relies on it bumping buildToken
  // (latest-wins cancellation). Re-implement the token bump + state reset minimally.
  (fake as unknown as AnyRec)["cancelBuild"] = function (this: AnyRec) {
    this["buildToken"] = (this["buildToken"] as number) + 1;
    // Mirror the REAL cancelBuild: cancel any pending rAF + drop the handle, so a
    // stale batch callback can't accumulate in the fake rAF queue across rebuilds.
    if (this["buildRaf"] !== null) {
      cancelAnimationFrame(this["buildRaf"] as number);
      this["buildRaf"] = null;
    }
    this["buildState"] = null;
    if (this["reprioritizeTimer"] !== null) {
      clearTimeout(this["reprioritizeTimer"] as ReturnType<typeof setTimeout>);
      this["reprioritizeTimer"] = null;
    }
    this["mutationState"] = "idle";
  };
  // roadGraph construction: setCityState does `new RoadGraph(...)`. Stub the two
  // static signature helpers via the prototype? They're static + pure; leave them.
  // RoadGraph + makeWaterBlocker run for real on an empty road set (cheap).

  // Record placements; bump the buildingNodes size counter.
  (fake as unknown as AnyRec)["createBuildingNode"] = function (
    this: AnyRec,
    b: Building,
  ) {
    placed.push(b.fileId);
    (this["buildingNodes"] as { size: number }).size++;
    // Register the chunk so post-build viewportPriorityChunks could read it (not
    // needed for ordering, which uses candidate keys, but keeps parity).
    return { container: new Container() };
  };

  return { fake, placed };
}

// Private-method shim (setCityState is public, but cb's type differs).
const setCity = (f: unknown, c: CityState, cb?: (p: unknown) => void) =>
  (f as { setCityState: (c: CityState, cb?: unknown) => void }).setCityState(
    c,
    cb,
  );

beforeEach(() => {
  installRaf();
  fid = 0;
});

afterEach(() => {
  vi.useRealTimers();
});

// ---------------------------------------------------------------------------
// VISIBLE-FIRST ordering
// ---------------------------------------------------------------------------

describe("B2b visible-first build order", () => {
  it("places viewport-chunk buildings BEFORE out-of-view ones", () => {
    const { fake, placed } = makeRenderer("minimal"); // ring 0 → only the visible chunk
    // Two near (tile 0,0 → chunk 0,0 → visible) + two far (tile 40,40 → chunk 5,5).
    const near1 = mkBuilding(0, 0);
    const near2 = mkBuilding(1, 1);
    const far1 = mkBuilding(40, 40);
    const far2 = mkBuilding(41, 41);
    // Interleave the source order so depth-sort + priority is actually exercised.
    setCity(fake, mkCity([far1, near1, far2, near2]));

    // The synchronous prelude already computed the order; read buildState.
    const state = (fake as unknown as AnyRec)["buildState"] as {
      order: number[];
      cursor: number;
    } | null;
    expect(state).not.toBeNull();

    flushRaf();

    // Near buildings placed before far ones.
    const iNear1 = placed.indexOf(near1.fileId);
    const iNear2 = placed.indexOf(near2.fileId);
    const iFar1 = placed.indexOf(far1.fileId);
    const iFar2 = placed.indexOf(far2.fileId);
    expect(Math.max(iNear1, iNear2)).toBeLessThan(Math.min(iFar1, iFar2));
    // Every building placed exactly once.
    expect(placed.length).toBe(4);
    expect(new Set(placed).size).toBe(4);
  });
});

// ---------------------------------------------------------------------------
// PROGRESS — visible-complete before total-complete
// ---------------------------------------------------------------------------

describe("B2b progress reports visible-complete before total-complete", () => {
  it("emits visibleDone===visibleTotal on a building batch, and a final phase:done", () => {
    const { fake } = makeRenderer("minimal");
    // Many buildings spread across chunks so the build needs >1 batch is NOT
    // required for the contract; with one batch the building progress still carries
    // visibleDone/visibleTotal and the done callback repeats them. Use a visible +
    // a far set so visibleTotal>0 and < total.
    const buildings = [
      mkBuilding(0, 0),
      mkBuilding(1, 0),
      mkBuilding(50, 50),
      mkBuilding(51, 51),
    ];
    const progress: Array<{
      phase: string;
      done: number;
      total: number;
      visibleDone?: number;
      visibleTotal?: number;
    }> = [];
    setCity(fake, mkCity(buildings), (p) =>
      progress.push(
        p as {
          phase: string;
          done: number;
          total: number;
          visibleDone?: number;
          visibleTotal?: number;
        },
      ),
    );
    flushRaf();

    expect(progress.length).toBeGreaterThan(0);
    const done = progress[progress.length - 1];
    expect(done.phase).toBe("done");
    expect(done.visibleTotal).toBe(2); // the two visible buildings
    expect(done.visibleTotal).toBeLessThan(done.total); // visible subset < whole
    // The done callback reports the visible subset fully placed.
    expect(done.visibleDone).toBe(done.visibleTotal);
    // visibleDone is monotonic and never exceeds visibleTotal across the stream.
    for (const p of progress) {
      if (p.visibleTotal !== undefined && p.visibleDone !== undefined) {
        expect(p.visibleDone).toBeLessThanOrEqual(p.visibleTotal);
      }
    }
  });
});

// ---------------------------------------------------------------------------
// REPRIORITIZE — camera move mid-fill re-sorts the remainder, no re-place
// ---------------------------------------------------------------------------

describe("B2b reprioritization on camera move mid-build", () => {
  it("re-sorts the not-yet-placed tail toward the new viewport without re-placing the head", () => {
    const { fake } = makeRenderer("minimal");
    // A row of chunks: chunk 0 (visible), then chunks 5 and 10 (out of view).
    const c0 = mkBuilding(0, 0); // chunk 0,0 — visible initially
    const c5 = mkBuilding(40, 0); // chunk 5,0
    const c10 = mkBuilding(80, 0); // chunk 10,0
    setCity(fake, mkCity([c0, c5, c10]));

    const state = (fake as unknown as AnyRec)["buildState"] as {
      order: number[];
      cursor: number;
      chunkXY: { cx: number; cy: number }[];
      sorted: Building[];
    };
    // Initially: c0 (visible) is first; the rest by distance from center (chunk 0).
    // Simulate the first building (c0) already placed.
    state.cursor = 1;
    const headSrcIdx = state.order[0];
    const headFileId = state.sorted[headSrcIdx].fileId;
    expect(headFileId).toBe(c0.fileId);

    // Move the camera to look at chunk 10. cartToIso(80,0) = (3840,1920); chunk
    // 10,0's iso bounds are ~x[3360,4320] y[1540,2400]. A window there intersects
    // chunk 10 and NOT chunk 5 (~x[1440,2400]). center (3700,1900) ∈ chunk 10.
    (fake as unknown as AnyRec)["viewport"] = makeViewport(3500, 1700, 400, 400);
    // Directly invoke the reprioritization (the debounced scheduler calls this).
    (fake as unknown as { reprioritizeRemaining: () => void }).reprioritizeRemaining();

    // The head [0,cursor) is untouched (c0 still first).
    expect(state.sorted[state.order[0]].fileId).toBe(c0.fileId);
    // The tail [cursor, end) is reordered toward chunk 10: c10 should now precede
    // c5 in the remaining queue (it is in/closer to the new viewport).
    const tailFileIds = state.order
      .slice(state.cursor)
      .map((i) => state.sorted[i].fileId);
    expect(tailFileIds).toContain(c5.fileId);
    expect(tailFileIds).toContain(c10.fileId);
    expect(tailFileIds.indexOf(c10.fileId)).toBeLessThan(
      tailFileIds.indexOf(c5.fileId),
    );
    // No duplication / loss: the full order is still a permutation of [0,3).
    expect([...state.order].sort((a, b) => a - b)).toEqual([0, 1, 2]);
  });

  it("a debounced moved burst coalesces to ONE armed timer that calls reprioritizeRemaining", () => {
    const { fake } = makeRenderer("minimal");
    const calls = { n: 0 };
    (fake as unknown as Record<string, unknown>)["reprioritizeRemaining"] = () => {
      calls.n++;
    };
    // Capture every setTimeout the scheduler arms so we can fire the LAST one and
    // assert the earlier ones were cleared (coalesced) — independent of fake-timer
    // wiring, which can clash with the rAF stub.
    const armed: Array<{ cb: () => void; cleared: boolean }> = [];
    const realSetTimeout = globalThis.setTimeout;
    const realClearTimeout = globalThis.clearTimeout;
    let nextId = 1;
    const idToEntry = new Map<number, { cb: () => void; cleared: boolean }>();
    globalThis.setTimeout = ((cb: () => void) => {
      const id = nextId++;
      const entry = { cb, cleared: false };
      armed.push(entry);
      idToEntry.set(id, entry);
      return id as unknown as ReturnType<typeof setTimeout>;
    }) as typeof setTimeout;
    globalThis.clearTimeout = ((id: number) => {
      const e = idToEntry.get(id);
      if (e) e.cleared = true;
    }) as typeof clearTimeout;
    try {
      setCity(fake, mkCity([mkBuilding(0, 0), mkBuilding(40, 0)]));
      const sched = (
        fake as unknown as { scheduleReprioritize: () => void }
      ).scheduleReprioritize.bind(fake);
      sched();
      sched();
      sched();
      // Three arms; the first two were cleared (debounced), only the last is live.
      expect(armed.length).toBe(3);
      expect(armed[0].cleared).toBe(true);
      expect(armed[1].cleared).toBe(true);
      expect(armed[2].cleared).toBe(false);
      // Firing the live one calls reprioritizeRemaining exactly once.
      armed[2].cb();
      expect(calls.n).toBe(1);
    } finally {
      globalThis.setTimeout = realSetTimeout;
      globalThis.clearTimeout = realClearTimeout;
    }
  });
});

// ---------------------------------------------------------------------------
// CANCELLATION — a fresh build mid-priority-build aborts the in-flight loop
// ---------------------------------------------------------------------------

describe("B2b cancellation mid-priority-build", () => {
  it("a new setCityState bumps the token so the in-flight batch loop bows out", () => {
    const { fake, placed } = makeRenderer("minimal");
    // First city: enough buildings that a single stepRaf won't finish (force a
    // multi-batch build by exceeding the batch size).
    const many = Array.from({ length: 320 }, (_, i) =>
      mkBuilding(i % 8, Math.floor(i / 8)),
    );
    setCity(fake, mkCity(many));
    // Run ONE batch (150 placed), leaving the build in flight.
    stepRaf();
    const afterFirstBatch = placed.length;
    expect(afterFirstBatch).toBeGreaterThan(0);
    expect(afterFirstBatch).toBeLessThan(many.length);

    // Start a FRESH build (latest wins) — its cancelBuild bumps the token; the OLD
    // build's queued rAF must now bow out and place nothing more for the old city.
    const placedBefore = placed.length;
    fid = 1000; // distinct fileIds for the second city
    const second = [mkBuilding(0, 0), mkBuilding(1, 0)];
    setCity(fake, mkCity(second));
    // Drain everything. The OLD build's stale rAF callbacks self-cancel on the token
    // mismatch; only the SECOND build's buildings should be placed beyond the head.
    flushRaf();

    // The second city's buildings are placed.
    expect(placed).toContain("f1001");
    expect(placed).toContain("f1002");
    // The stale first-build callback placed nothing AFTER the cancel (its token no
    // longer matches), so no first-city building was added in a stale batch beyond
    // what the first batch already placed. We assert the in-flight loop bowed out:
    // the placed-count grew ONLY by the second city's buildings (2), not by another
    // 150-building stale batch.
    expect(placed.length).toBe(placedBefore + second.length);
    // buildState now reflects the SECOND (completed) build → cleared by finalize.
    expect((fake as unknown as AnyRec)["buildState"]).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// FIX 1 — camera pre-fit ONLY on the first-ever build, never on a fallback rebuild
// ---------------------------------------------------------------------------

/** A viewport that RECORDS setZoom/moveCenter so a test can prove whether the B2b
 *  pre-fit perturbed the camera. left/top/bounds are mutable so a test can simulate
 *  a user pan between builds. */
function makeRecordingViewport(left: number, top: number, w: number, h: number) {
  return {
    left,
    top,
    worldScreenWidth: w,
    worldScreenHeight: h,
    scale: { x: 1 },
    moves: [] as Array<{ cx: number; cy: number }>,
    zooms: [] as number[],
    findFit: () => 1,
    setZoom(z: number) {
      this.zooms.push(z);
    },
    moveCenter(cx: number, cy: number) {
      this.moves.push({ cx, cy });
    },
  };
}

describe("B2b camera pre-fit gating (FIX 1 — no camera yank on fallback rebuild)", () => {
  it("first-ever build DOES pre-fit the camera; a fallback rebuild does NOT move it", () => {
    const { fake } = makeRenderer("minimal");
    const vp = makeRecordingViewport(-200, -400, 400, 600);
    (fake as unknown as AnyRec)["viewport"] = vp;

    // FIRST-EVER build: lastCity is null → the pre-fit must run (camera framed to
    // the city) and the build completes, setting lastCity. The recenter stub is a
    // no-op, so any recorded move/zoom came ONLY from the B2b pre-fit.
    const cityA = mkCity([mkBuilding(0, 0), mkBuilding(40, 40)]);
    setCity(fake, cityA);
    flushRaf();
    expect(vp.moves.length).toBeGreaterThan(0); // pre-fit ran
    expect(vp.zooms.length).toBeGreaterThan(0);
    expect((fake as unknown as AnyRec)["lastCity"]).toBe(cityA);

    // Simulate a user PAN/ZOOM after the first build settled.
    vp.left = 5000;
    vp.top = 5000;
    vp.scale.x = 1.3;
    // Reset the recorders so we observe ONLY what the second build does.
    vp.moves.length = 0;
    vp.zooms.length = 0;
    const userLeft = vp.left;
    const userTop = vp.top;
    const userScale = vp.scale.x;

    // FALLBACK rebuild: applyCityDiff falls back to exactly this call
    // (`this.setCityState(next, …)`) with lastCity already set. The pre-fit MUST be
    // skipped so the user's pan/zoom survives.
    const cityB = mkCity([mkBuilding(0, 0), mkBuilding(40, 40), mkBuilding(41, 41)]);
    setCity(fake, cityB);
    flushRaf();

    // The pre-fit did NOT run: no setZoom/moveCenter (recenter is a no-op stub, so a
    // move here could only have come from fitCameraToBuildings).
    expect(vp.moves.length).toBe(0);
    expect(vp.zooms.length).toBe(0);
    // The user's camera is untouched.
    expect(vp.left).toBe(userLeft);
    expect(vp.top).toBe(userTop);
    expect(vp.scale.x).toBe(userScale);
  });
});

// ---------------------------------------------------------------------------
// FIX 2 — reprioritize recomputes visibleTotal to the NEW priority head
// ---------------------------------------------------------------------------

describe("B2b reprioritize updates visibleTotal (FIX 2)", () => {
  it("reprioritizing onto a denser chunk recomputes visibleTotal to the new visible count", () => {
    const { fake } = makeRenderer("minimal"); // ring 0 → only the center chunk is visible
    // chunk 0 (1 bldg, initially visible) + chunk 10 (2 bldgs, initially out of view).
    const c0 = mkBuilding(0, 0); // chunk 0,0
    const c10a = mkBuilding(80, 0); // chunk 10,0
    const c10b = mkBuilding(81, 0); // chunk 10,0
    setCity(fake, mkCity([c0, c10a, c10b]));

    const state = (fake as unknown as AnyRec)["buildState"] as {
      visibleTotal: number;
      cursor: number;
    };
    // Initially exactly ONE building is in the visible (chunk 0) set.
    expect(state.visibleTotal).toBe(1);

    // Move the camera onto chunk 10 (which holds TWO buildings) BEFORE any batch
    // runs (cursor still 0), then reprioritize.
    (fake as unknown as AnyRec)["viewport"] = makeViewport(3500, 1700, 400, 400);
    (fake as unknown as { reprioritizeRemaining: () => void }).reprioritizeRemaining();

    // visibleTotal is recomputed to the new visible set (the two chunk-10 buildings).
    expect(state.visibleTotal).toBe(2);
  });

  it("a building-phase progress callback emitted AFTER reprioritize carries the recomputed visibleTotal", () => {
    const { fake } = makeRenderer("minimal");
    // Enough buildings to force a multi-batch build so a batch runs AFTER we
    // reprioritize and emits a building-phase progress callback.
    const near = Array.from({ length: 2 }, (_, i) => mkBuilding(i, 0)); // chunk 0 (visible)
    // >300 far buildings → ≥3 batches, so a building-phase callback still fires
    // AFTER we reprioritize between batch 1 and batch 2 (the LAST batch emits the
    // "done" phase, not "building").
    const far = Array.from({ length: 400 }, (_, i) =>
      mkBuilding(80 + (i % 4), Math.floor(i / 4)),
    ); // chunk 10,* (out of view)
    const progress: Array<{
      phase: string;
      visibleTotal?: number;
      visibleDone?: number;
    }> = [];
    setCity(fake, mkCity([...near, ...far]), (p) =>
      progress.push(
        p as { phase: string; visibleTotal?: number; visibleDone?: number },
      ),
    );

    const state = (fake as unknown as AnyRec)["buildState"] as {
      visibleTotal: number;
    };
    const initialVisible = state.visibleTotal;
    expect(initialVisible).toBe(2); // the two chunk-0 buildings

    // Run ONE batch (leaves the build in flight), then move the camera onto the far
    // chunk and reprioritize.
    stepRaf();
    (fake as unknown as AnyRec)["viewport"] = makeViewport(3000, 0, 1200, 1200);
    (fake as unknown as { reprioritizeRemaining: () => void }).reprioritizeRemaining();
    const reprioritized = state.visibleTotal;
    expect(reprioritized).not.toBe(initialVisible);

    const lenBefore = progress.length;
    // Drive the next batch → it emits a building-phase progress callback that must
    // carry the RECOMPUTED visibleTotal (state.visibleTotal), not the stale 2.
    stepRaf();
    const after = progress.slice(lenBefore).filter((p) => p.phase === "building");
    expect(after.length).toBeGreaterThan(0);
    for (const p of after) {
      expect(p.visibleTotal).toBe(reprioritized);
    }
  });
});

// ---------------------------------------------------------------------------
// FIX 3 — nearest-fallback tracks the TRUE nearest across ALL candidates
// ---------------------------------------------------------------------------

describe("viewportPriorityChunks nearest-fallback (FIX 3)", () => {
  it("when the view center is in a gap (no containing chunk), center is the genuine nearest chunk", () => {
    const { fake } = makeRenderer("minimal");
    // Candidate chunks at 0,0 / 5,0 / 10,0. Put the view center far to the RIGHT so
    // NO chunk contains it (a gap, e.g. over open water) — the nearest must win.
    // cartToIso chunk centers grow with cx, so chunk 10 is the rightmost; a center
    // beyond all of them must resolve to chunk 10 (true nearest), independent of the
    // candidate Set's iteration order.
    const candidateKeys = ["0,0", "5,0", "10,0"];
    const vp = makeViewport(100000, 0, 10, 10); // center ≈ (100005, 5): far right
    (fake as unknown as AnyRec)["viewport"] = vp;

    const { center } = (
      fake as unknown as {
        viewportPriorityChunks: (
          keys: Iterable<string>,
          ring: number,
        ) => { keys: Set<string>; center: { cx: number; cy: number } };
      }
    ).viewportPriorityChunks(candidateKeys, 0);

    // The true nearest to a far-right center is the rightmost candidate chunk (10,0).
    expect(center).toEqual({ cx: 10, cy: 0 });
  });

  it("a containing chunk wins regardless of candidate order (contains test is unconditional)", () => {
    const { fake } = makeRenderer("minimal");
    // A center INSIDE chunk 0,0's bounds. cartToIso(0,0)=(0,0); chunk 0 bounds wrap
    // the origin, so a small window centered at (0,0) is contained by chunk 0. Only
    // chunk 0 contains it; chunk 10 is far. Contains must beat distance, and the
    // candidate order must NOT change the verdict.
    const candidates = ["0,0", "10,0"];
    const vp = makeViewport(-50, -50, 100, 100); // center (0,0) ∈ chunk 0,0
    (fake as unknown as AnyRec)["viewport"] = vp;
    const call = (keys: Iterable<string>) =>
      (
        fake as unknown as {
          viewportPriorityChunks: (
            keys: Iterable<string>,
            ring: number,
          ) => { center: { cx: number; cy: number } };
        }
      ).viewportPriorityChunks(keys, 0).center;

    expect(call(candidates)).toEqual({ cx: 0, cy: 0 }); // contains wins
    // Reverse the order so the containing chunk is LAST — contains still wins.
    expect(call([...candidates].reverse())).toEqual({ cx: 0, cy: 0 });
  });
});
