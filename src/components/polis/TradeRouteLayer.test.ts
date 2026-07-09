import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { Container, Rectangle } from "pixi.js";
import { cartToIso, type IsoPoint } from "./iso";
import { TradeRouteLayer } from "./TradeRouteLayer";
import type { Road } from "../../types/city";
import * as people from "./kitcd/people";

// These tests exercise the DATA-BINDING + pooling + LOD/cull discipline of the
// trade-route porter layer WITHOUT a WebGL renderer. PIXI v8 Container/Graphics
// are plain scene-graph objects (no GL needed to construct + mutate transform/
// visibility), so the layer drives headlessly — exactly like growthEffects.test.

const VIEW = new Rectangle(-100000, -100000, 200000, 200000); // contains all
const OFFSCREEN = new Rectangle(1e9, 1e9, 1, 1); // contains nothing

function mkRoad(
  from: string,
  to: string,
  weight: number,
  path: { x: number; y: number }[],
): Road {
  return {
    roadId: `${from}->${to}`,
    from,
    to,
    type: "import",
    style: "lastricata",
    weight,
    path,
  };
}

// Resolve EVERY fileId to an arbitrary on-map position (the layer only uses this
// to confirm endpoints exist; geometry comes from the routed path).
const resolveAll = (): IsoPoint => ({ x: 0, y: 0 });

describe("TradeRouteLayer — threshold (top-weight edges only)", () => {
  it("spawns porters on heavy edges and skips the weight-1 long tail", () => {
    const root = new Container();
    const layer = new TradeRouteLayer(root);
    // One heavy edge (weight 5, qualifies by weight>=3) and many weight-1 lanes
    // that must NOT get porters (they'd be the long-tail noise). With 30 such
    // lanes the top-N (24) would otherwise sweep some in — so we keep the count
    // here at exactly the heavy edge to assert the WEIGHT gate, and test top-N
    // separately below.
    const roads: Road[] = [
      mkRoad("A", "B", 5, [
        { x: 0, y: 0 },
        { x: 4, y: 0 },
      ]),
    ];
    layer.setWorld(roads, resolveAll);
    // weight 5 → porterCountForWeight = 1 + floor(4/2) = 3 porters.
    expect(layer.count).toBe(3);
    layer.clear();
  });

  it("excludes weight-1 edges beyond the top-N cap", () => {
    const root = new Container();
    const layer = new TradeRouteLayer(root);
    // 40 weight-1 edges. None qualifies by weight (<3). Only the top-N (24) by
    // weight are kept; with all weights equal the tie-break is roadId, so 24
    // edges get exactly 1 porter each (weight 1 → 1 porter).
    const roads: Road[] = [];
    for (let i = 0; i < 40; i++) {
      roads.push(
        mkRoad(`S${i}`, `T${i}`, 1, [
          { x: i, y: 0 },
          { x: i, y: 2 },
        ]),
      );
    }
    layer.setWorld(roads, resolveAll);
    expect(layer.count).toBe(24); // exactly TRADE_TOP_N edges, 1 porter each
    layer.clear();
  });
});

describe("TradeRouteLayer — weight→porter mapping + global cap", () => {
  it("scales porters with weight, capped per edge", () => {
    const root = new Container();
    const layer = new TradeRouteLayer(root);
    // weight 1→1, 3→2, 5→3, 9→4 (capped). Use distinct heavy edges.
    layer.setWorld(
      [mkRoad("A", "B", 9, [{ x: 0, y: 0 }, { x: 1, y: 0 }])],
      resolveAll,
    );
    expect(layer.count).toBe(4); // capped at TRADE_PORTERS_PER_EDGE_CAP
    layer.clear();
  });

  it("never exceeds the global porter cap on a dense heavy graph", () => {
    const root = new Container();
    const layer = new TradeRouteLayer(root);
    // 100 heavy edges (weight 9 → 4 porters each = 400 wanted) must clamp to the
    // global cap of 80.
    const roads: Road[] = [];
    for (let i = 0; i < 100; i++) {
      roads.push(
        mkRoad(`H${i}`, `K${i}`, 9, [
          { x: i, y: 0 },
          { x: i, y: 1 },
        ]),
      );
    }
    layer.setWorld(roads, resolveAll);
    expect(layer.count).toBe(80);
    expect(root.children.length).toBe(80); // one container per porter, pooled
    layer.clear();
  });
});

describe("TradeRouteLayer — supplier→consumer direction", () => {
  it("walks from the supplier (road.to) toward the consumer (road.from)", () => {
    const root = new Container();
    const layer = new TradeRouteLayer(root);
    // Road A imports B: from=A (consumer), to=B (supplier). Routed path is stored
    // CONSUMER→SUPPLIER (from→to): A at cart (0,0), B at cart (10,0). The porter
    // must walk SUPPLIER→CONSUMER, i.e. from B's iso end TOWARD A's iso end.
    const path = [
      { x: 0, y: 0 }, // A / consumer (path[0])
      { x: 10, y: 0 }, // B / supplier (path[last])
    ];
    layer.setWorld([mkRoad("A", "B", 3, path)], resolveAll);
    layer.setLodVisible(true);

    const isoA = cartToIso(0, 0); // consumer end (the porter's DESTINATION)
    const isoB = cartToIso(10, 0); // supplier end (the porter's ORIGIN)
    // The reversed (walk) route runs isoB → isoA. isoA.x < isoB.x, so walking
    // SUPPLIER → CONSUMER means each porter's x must DECREASE over time. Capture
    // the start, advance a short distance (small enough that no porter wraps the
    // whole route), and assert x decreased — the supplier→consumer direction.
    expect(isoA.x).toBeLessThan(isoB.x);
    const before = root.children.map((c) => c.position.x);
    layer.update(50); // 0.05s @ 40px/s ≈ 2px — well short of the 480px route
    const after = root.children.map((c) => c.position.x);
    for (let i = 0; i < before.length; i++) {
      // x strictly decreased (moving toward the consumer end), and stays on the
      // segment between the two ends.
      expect(after[i]).toBeLessThan(before[i]);
      expect(after[i]).toBeGreaterThanOrEqual(isoA.x - 1);
      expect(after[i]).toBeLessThanOrEqual(isoB.x + 1);
    }
    layer.clear();
  });
});

describe("TradeRouteLayer — porters spread across the WHOLE route", () => {
  it("spreads a weight-3 edge's two porters over the route, not the first quarter", () => {
    const root = new Container();
    const layer = new TradeRouteLayer(root);
    // weight 3 → porterCountForWeight = 1 + floor(2/2) = 2 porters. A single long
    // straight segment so iso x is a monotone proxy for the route fraction.
    // Routed path is CONSUMER→SUPPLIER; the layer reverses it to SUPPLIER→CONSUMER
    // (path[0] = supplier iso, path[last] = consumer iso), so position fraction
    // along the WALK route = (xStart - x) / (xStart - xEnd).
    const path = [
      { x: 0, y: 0 }, // consumer (path[0] in storage)
      { x: 100, y: 0 }, // supplier (path[last] in storage)
    ];
    layer.setWorld([mkRoad("A", "B", 3, path)], resolveAll);
    expect(layer.count).toBe(2);

    const isoStart = cartToIso(100, 0); // supplier end = porter ORIGIN (frac 0)
    const isoEnd = cartToIso(0, 0); // consumer end = porter DESTINATION (frac 1)
    const span = isoStart.x - isoEnd.x; // > 0 (start.x > end.x)
    expect(span).toBeGreaterThan(0);

    const fracs = root.children
      .map((c) => (isoStart.x - c.position.x) / span)
      .sort((p, q) => p - q);
    // Two porters: one in the first half, the other in the second half — NOT
    // both bunched in the first part of the route. This is the red→green
    // discriminator: with the old divide-by-cap (4) the seeded fracs are 0.127
    // and 0.316 (both < 0.5, second porter stuck at ~1/3); with the divide-by-
    // actual-count (2) fix they are 0.254 and 0.632 — spread across the whole
    // route. The `fracs[1] >= 0.5` assertion FAILS on the old code, PASSES now.
    expect(fracs[0]).toBeLessThan(0.5);
    expect(fracs[1]).toBeGreaterThanOrEqual(0.5);
    layer.clear();
  });

  it("is deterministic — same seed yields the same spread", () => {
    const mk = () => {
      const root = new Container();
      const layer = new TradeRouteLayer(root);
      layer.setWorld(
        [mkRoad("A", "B", 3, [{ x: 0, y: 0 }, { x: 100, y: 0 }])],
        resolveAll,
      );
      const xs = root.children.map((c) => c.position.x).sort((a, b) => a - b);
      layer.clear();
      return xs;
    };
    expect(mk()).toEqual(mk());
  });
});

describe("TradeRouteLayer — short-path loop-back consumes leftover (no teleport)", () => {
  it("wraps smoothly when a frame step exceeds the path length", () => {
    const root = new Container();
    const layer = new TradeRouteLayer(root);
    // Tiny route: iso length is small relative to one big step. weight 1 → 1
    // porter so we can track it precisely.
    layer.setWorld(
      [mkRoad("A", "B", 1, [{ x: 0, y: 0 }, { x: 1, y: 0 }])],
      resolveAll,
    );
    layer.setLodVisible(true);
    expect(layer.count).toBe(1);
    const c = root.children[0] as Container;
    // A huge delta (10s @ 40px/s = 400px) dwarfs the ~64px iso route → the porter
    // must keep walking past the wrap, landing somewhere finite on the route, not
    // NaN and not stuck.
    for (let i = 0; i < 5; i++) {
      layer.update(10000);
      expect(Number.isFinite(c.position.x)).toBe(true);
      expect(Number.isFinite(c.position.y)).toBe(true);
    }
    // Position stays within the iso route's bounding box (with a small margin).
    const isoA = cartToIso(0, 0);
    const isoB = cartToIso(1, 0);
    const lo = Math.min(isoA.x, isoB.x) - 1;
    const hi = Math.max(isoA.x, isoB.x) + 1;
    expect(c.position.x).toBeGreaterThanOrEqual(lo);
    expect(c.position.x).toBeLessThanOrEqual(hi);
    layer.clear();
  });

  it("caps per-call deltaMs so a long background stall can't over-advance or freeze (FIX 4)", () => {
    // A long route + an enormous deltaMs (tab backgrounded for ~167 minutes).
    // Without the cap the porter would consume ~400_000px of travel in ~1e-6px
    // steps on a tiny route (main-thread freeze) OR teleport far on a long one.
    // With the cap, ONE update(huge) advances exactly as far as update(50) — the
    // 50ms (MAX_STEP_MS) bound — and never more.
    const mk = () => {
      const root = new Container();
      const layer = new TradeRouteLayer(root);
      // One long straight segment, weight 1 → exactly one porter to track.
      layer.setWorld(
        [mkRoad("A", "B", 1, [{ x: 0, y: 0 }, { x: 10000, y: 0 }])],
        resolveAll,
      );
      layer.setLodVisible(true);
      return { root, layer };
    };
    // Reference: a single capped step (50ms).
    const ref = mk();
    const refStart = (ref.root.children[0] as Container).position.x;
    ref.layer.update(50);
    const refDelta = Math.abs(
      (ref.root.children[0] as Container).position.x - refStart,
    );
    ref.layer.clear();
    // Subject: a single update with an absurd delta. It must advance the SAME
    // capped distance, not proportionally more.
    const sub = mk();
    const subStart = (sub.root.children[0] as Container).position.x;
    sub.layer.update(10_000_000); // ~167 min stall
    const c = sub.root.children[0] as Container;
    expect(Number.isFinite(c.position.x)).toBe(true);
    const subDelta = Math.abs(c.position.x - subStart);
    // Capped: the huge-delta advance equals the 50ms advance (within fp slack).
    expect(subDelta).toBeCloseTo(refDelta, 4);
    sub.layer.clear();
  });

  it("does not loop forever / NaN on a zero-length (degenerate) path", () => {
    const root = new Container();
    const layer = new TradeRouteLayer(root);
    // Both waypoints coincide → routeLen 0. Must seat the porter and return.
    layer.setWorld(
      [mkRoad("A", "B", 1, [{ x: 5, y: 5 }, { x: 5, y: 5 }])],
      resolveAll,
    );
    layer.setLodVisible(true);
    expect(layer.count).toBe(1);
    const c = root.children[0] as Container;
    layer.update(10000); // would spin forever if unguarded
    const iso = cartToIso(5, 5);
    expect(Number.isFinite(c.position.x)).toBe(true);
    expect(c.position.x).toBeCloseTo(iso.x, 5);
    expect(c.position.y).toBeCloseTo(iso.y + -4, 5); // OMINO_Y_OFFSET = -4
    layer.clear();
  });
});

describe("TradeRouteLayer — ZOOM-IN ONLY LOD gate", () => {
  it("draws no porter while zoomed out (below the threshold)", () => {
    const root = new Container();
    const layer = new TradeRouteLayer(root);
    layer.setWorld(
      [mkRoad("A", "B", 5, [{ x: 0, y: 0 }, { x: 4, y: 0 }])],
      resolveAll,
    );
    // Zoomed out: layer hidden — step is a no-op, every porter container stays
    // invisible, update doesn't move anything.
    layer.setLodVisible(false);
    layer.update(500);
    layer.step(3, VIEW);
    for (const child of root.children) expect(child.visible).toBe(false);
    layer.clear();
  });

  it("shows on-screen porters once zoomed in", () => {
    const root = new Container();
    const layer = new TradeRouteLayer(root);
    layer.setWorld(
      [mkRoad("A", "B", 5, [{ x: 0, y: 0 }, { x: 4, y: 0 }])],
      resolveAll,
    );
    layer.setLodVisible(true);
    layer.step(0, VIEW);
    const anyVisible = root.children.some((c) => c.visible);
    expect(anyVisible).toBe(true);
    layer.clear();
  });
});

describe("TradeRouteLayer — visible-chunk cull", () => {
  it("hides porters outside the visible bounds even when zoomed in", () => {
    const root = new Container();
    const layer = new TradeRouteLayer(root);
    layer.setWorld(
      [mkRoad("A", "B", 5, [{ x: 0, y: 0 }, { x: 4, y: 0 }])],
      resolveAll,
    );
    layer.setLodVisible(true);
    layer.step(0, OFFSCREEN); // nothing in view
    for (const child of root.children) expect(child.visible).toBe(false);
    // Bring the camera onto them → they show again.
    layer.step(0, VIEW);
    expect(root.children.some((c) => c.visible)).toBe(true);
    layer.clear();
  });
});

describe("TradeRouteLayer — click surfaces the real connection", () => {
  it("reports from (consumer) and to (supplier) on a porter tap", () => {
    const root = new Container();
    let got: { from: string; to: string } | null = null;
    const layer = new TradeRouteLayer(root, (from, to) => {
      got = { from, to };
    });
    layer.setWorld(
      [mkRoad("consumer.ts", "supplier.ts", 5, [{ x: 0, y: 0 }, { x: 4, y: 0 }])],
      resolveAll,
    );
    // Fire a pointertap on the first porter container.
    const porter = root.children[0] as Container;
    porter.emit("pointertap", { stopPropagation() {} } as never);
    expect(got).toEqual({ from: "consumer.ts", to: "supplier.ts" });
    layer.clear();
  });
});

describe("TradeRouteLayer — teardown + rebuild", () => {
  it("clear() detaches + frees every porter (no leak)", () => {
    const root = new Container();
    const layer = new TradeRouteLayer(root);
    layer.setWorld(
      [mkRoad("A", "B", 9, [{ x: 0, y: 0 }, { x: 4, y: 0 }])],
      resolveAll,
    );
    expect(root.children.length).toBeGreaterThan(0);
    layer.clear();
    expect(root.children.length).toBe(0);
    expect(layer.count).toBe(0);
  });

  it("rebuilds cleanly on a fresh setWorld (no orphan children)", () => {
    const root = new Container();
    const layer = new TradeRouteLayer(root);
    layer.setWorld(
      [mkRoad("A", "B", 9, [{ x: 0, y: 0 }, { x: 4, y: 0 }])],
      resolveAll,
    );
    const first = layer.count;
    expect(first).toBe(4);
    // A second setWorld must tear down the previous pool first (no accumulation).
    layer.setWorld(
      [mkRoad("C", "D", 5, [{ x: 0, y: 0 }, { x: 2, y: 0 }])],
      resolveAll,
    );
    expect(layer.count).toBe(3); // weight 5 → 3, NOT 4+3
    expect(root.children.length).toBe(3);
    layer.clear();
  });

  it("skips edges with no routed path or a missing endpoint (data-bound only)", () => {
    const root = new Container();
    const layer = new TradeRouteLayer(root);
    const roads: Road[] = [
      mkRoad("A", "B", 9, []), // no routed polyline → skipped
      mkRoad("X", "X", 9, [{ x: 0, y: 0 }, { x: 1, y: 0 }]), // self-loop → skipped
    ];
    // A resolver that only knows A and B (not the heavy edge's path anyway).
    layer.setWorld(roads, (id) => (id === "A" || id === "B" ? { x: 0, y: 0 } : null));
    expect(layer.count).toBe(0);
    layer.clear();
  });

  it("skips an edge whose endpoint is not on the map", () => {
    const root = new Container();
    const layer = new TradeRouteLayer(root);
    layer.setWorld(
      [mkRoad("A", "GONE", 9, [{ x: 0, y: 0 }, { x: 4, y: 0 }])],
      (id) => (id === "A" ? { x: 0, y: 0 } : null), // GONE unresolved
    );
    expect(layer.count).toBe(0);
    layer.clear();
  });
});

describe("TradeRouteLayer — porters draw with carrying: crate", () => {
  let spy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    // Spy on drawCitizen to capture the opts passed by the layer.
    spy = vi.spyOn(people, "drawCitizen").mockImplementation(() => {});
  });

  afterEach(() => {
    spy.mockRestore();
  });

  it("passes carrying: crate at both redraw and spawn call sites", () => {
    const root = new Container();
    const layer = new TradeRouteLayer(root);
    layer.setWorld(
      [mkRoad("A", "B", 5, [{ x: 0, y: 0 }, { x: 4, y: 0 }])],
      resolveAll,
    );
    layer.setLodVisible(true);
    // Step triggers the per-frame redraw path (call site 1).
    layer.step(0, VIEW);
    // update triggers the per-frame redraw path too.
    layer.update(50);
    // Every call to drawCitizen from the layer must include carrying: "crate".
    for (const call of spy.mock.calls) {
      const opts = call[2] as Record<string, unknown>;
      expect(opts.carrying).toBe("crate");
    }
    // At least one call must have been made.
    expect(spy.mock.calls.length).toBeGreaterThan(0);
    layer.clear();
  });
});
