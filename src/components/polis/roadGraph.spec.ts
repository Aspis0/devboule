// Self-contained spec for the pure Polis road-navigation graph (`roadGraph.ts`).
//
// Same pattern as `diffCity.spec.ts`: the project has NO JS test runner wired
// (package.json scripts are only dev/build/preview/tauri), so this is a
// ZERO-DEPENDENCY, self-asserting module that exports `runRoadGraphSpec()` which
// throws on any failed assertion. It type-checks as part of the normal build
// (the code under test is pure) and can be invoked from a scratch script or a
// future runner without change.
//
// Contract verified:
//   - a path across 2 connected roads → waypoints concatenated, correct
//     orientation, junction de-duplicated,
//   - reversed orientation when a road is traversed to→from,
//   - no path to a disconnected node → null,
//   - no path when an endpoint has no roads → null,
//   - from === to → null (no travel),
//   - deterministic: identical route across repeated calls,
//   - the visited cap is respected (a chain longer than the cap → null).

import { RoadGraph, ROUTE_VISIT_CAP } from "./roadGraph";
import { cartToIso } from "./iso";
import type { Road } from "../../types/city";

function mkRoad(
  from: string,
  to: string,
  path: { x: number; y: number }[],
): Road {
  return {
    roadId: `${from}->${to}`,
    from,
    to,
    type: "import",
    style: "lastricata",
    weight: 1,
    path,
  };
}

function assert(cond: boolean, msg: string): void {
  if (!cond) throw new Error(`roadGraph spec failed: ${msg}`);
}

function approx(a: number, b: number): boolean {
  return Math.abs(a - b) < 1e-6;
}

/** Assert two ISO waypoint lists are equal point-for-point. */
function eqPoints(
  got: { x: number; y: number }[] | null,
  want: { x: number; y: number }[],
  msg: string,
): void {
  assert(got !== null, `${msg}: expected a route, got null`);
  const g = got as { x: number; y: number }[];
  assert(
    g.length === want.length,
    `${msg}: length ${g.length} !== ${want.length}`,
  );
  for (let i = 0; i < want.length; i++) {
    assert(
      approx(g[i].x, want[i].x) && approx(g[i].y, want[i].y),
      `${msg}: point ${i} (${g[i].x},${g[i].y}) !== (${want[i].x},${want[i].y})`,
    );
  }
}

/** Runs every road-graph assertion. Throws on the first failure; returns
 *  silently on success. Pure — no IO, no globals, no PIXI. */
export function runRoadGraphSpec(): void {
  // Layout (cartesian tiles):
  //   A(0,0) --[road1]--> B(2,0) --[road2]--> C(4,0)
  //   D(0,4) is isolated (one road to nowhere reachable from A/B/C).
  // road1 path: A=(0,0) -> (1,0) -> B=(2,0)
  // road2 path: B=(2,0) -> (3,0) -> C=(4,0)
  const road1 = mkRoad("A", "B", [
    { x: 0, y: 0 },
    { x: 1, y: 0 },
    { x: 2, y: 0 },
  ]);
  const road2 = mkRoad("B", "C", [
    { x: 2, y: 0 },
    { x: 3, y: 0 },
    { x: 4, y: 0 },
  ]);
  // D connects only to a node E that is NOT on the A/B/C network → separate
  // component, so A→D must be null.
  const road3 = mkRoad("D", "E", [
    { x: 0, y: 4 },
    { x: 1, y: 4 },
  ]);

  const graph = new RoadGraph([road1, road2, road3]);

  // --- 1) A → C: two roads concatenated, junction de-duplicated, ISO-converted.
  // Expected cartesian waypoint chain: (0,0),(1,0),(2,0),(3,0),(4,0)
  //   road1 contributes all of its points; road2 drops its shared first point.
  {
    const route = graph.findRoute("A", "C");
    const wantCart = [
      { x: 0, y: 0 },
      { x: 1, y: 0 },
      { x: 2, y: 0 },
      { x: 3, y: 0 },
      { x: 4, y: 0 },
    ];
    const wantIso = wantCart.map((p) => cartToIso(p.x, p.y));
    eqPoints(route, wantIso, "A→C two-road concat");
  }

  // --- 2) Orientation: C → A walks the SAME street backwards. Expected cart
  // chain reversed: (4,0),(3,0),(2,0),(1,0),(0,0).
  {
    const route = graph.findRoute("C", "A");
    const wantCart = [
      { x: 4, y: 0 },
      { x: 3, y: 0 },
      { x: 2, y: 0 },
      { x: 1, y: 0 },
      { x: 0, y: 0 },
    ];
    const wantIso = wantCart.map((p) => cartToIso(p.x, p.y));
    eqPoints(route, wantIso, "C→A reversed orientation");
  }

  // --- 3) Single road A → B: just road1's polyline, ISO-converted.
  {
    const route = graph.findRoute("A", "B");
    const wantIso = [
      cartToIso(0, 0),
      cartToIso(1, 0),
      cartToIso(2, 0),
    ];
    eqPoints(route, wantIso, "A→B single road");
  }

  // --- 4) Disconnected node: A → D is in a different component → null.
  assert(graph.findRoute("A", "D") === null, "A→D disconnected → null");

  // --- 5) Endpoint with no roads at all → null (not a node in the graph).
  assert(graph.findRoute("A", "Z") === null, "A→Z (Z has no roads) → null");
  assert(graph.has("Z") === false, "Z is not a graph node");
  assert(graph.has("A") === true, "A is a graph node");

  // --- 6) from === to → null (no travel; caller keeps in-place pose).
  assert(graph.findRoute("A", "A") === null, "A→A no travel → null");

  // --- 7) Determinism: identical route across repeated calls.
  {
    const r1 = graph.findRoute("A", "C");
    const r2 = graph.findRoute("A", "C");
    eqPoints(
      r1,
      (r2 ?? []) as { x: number; y: number }[],
      "deterministic A→C repeat",
    );
  }

  // --- 8) Visited cap respected: a chain LONGER than the cap returns null even
  // though a path topologically exists. Build N0-N1-...-Nk with k > cap.
  {
    const chain: Road[] = [];
    const len = ROUTE_VISIT_CAP + 50;
    for (let i = 0; i < len; i++) {
      chain.push(
        mkRoad(`N${i}`, `N${i + 1}`, [
          { x: i, y: 0 },
          { x: i + 1, y: 0 },
        ]),
      );
    }
    const big = new RoadGraph(chain);
    assert(
      big.findRoute("N0", `N${len}`) === null,
      "chain longer than visit cap → null",
    );
    // A SHORT hop in the same graph still works (cap is about total expansions).
    assert(big.findRoute("N0", "N1") !== null, "short hop within cap still ok");
  }
}
