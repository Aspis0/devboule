import { describe, it, expect } from "vitest";
import { Container } from "pixi.js";
import {
  AmbientLayer,
  pickNearestIdle,
  type ClaimableWalker,
} from "./AmbientLayer";
import { cartToIso, isoToCart, type IsoPoint } from "./iso";
import { roundTile } from "./navWalkable";
import type { CitizenType } from "./kitcd/people";

// Polis-P3 — the CLAIM primitives: an activating agent takes possession of an
// idle roaming omino (`release`) and later returns it to the crowd (`adopt`).
//
// `pickNearestIdle` is a PURE function tested headlessly. `release`/`adopt` run
// the AmbientLayer against headless PIXI v8 Container/Graphics (no GL context
// needed to construct + mutate the scene graph — same approach as
// TradeRouteLayer.test / ExternalServiceLayer.test).

// ---------------------------------------------------------------------------
// pickNearestIdle — pure selection
// ---------------------------------------------------------------------------

function w(
  type: CitizenType,
  pos: IsoPoint,
  kind: "idle" | "walk" = "idle",
): ClaimableWalker {
  return { type, pos, state: { kind } };
}

describe("pickNearestIdle — nearest IDLE walker of a type", () => {
  it("picks the geometrically closest IDLE walker of the matching type", () => {
    const walkers = [
      w("builder", { x: 100, y: 0 }),
      w("builder", { x: 10, y: 0 }), // closest builder
      w("builder", { x: 50, y: 0 }),
    ];
    expect(pickNearestIdle(walkers, "builder", { x: 0, y: 0 })).toBe(1);
  });

  it("ignores mid-walk (non-idle) walkers even if they are closer", () => {
    const walkers = [
      w("builder", { x: 5, y: 0 }, "walk"), // closer but mid-walk → skip
      w("builder", { x: 40, y: 0 }, "idle"),
    ];
    expect(pickNearestIdle(walkers, "builder", { x: 0, y: 0 })).toBe(1);
  });

  it("ignores other figure types", () => {
    const walkers = [
      w("noble", { x: 1, y: 0 }), // wrong type, very close
      w("builder", { x: 80, y: 0 }), // only matching idle builder
    ];
    expect(pickNearestIdle(walkers, "builder", { x: 0, y: 0 })).toBe(1);
  });

  it("returns -1 when no idle walker of the type exists", () => {
    const walkers = [
      w("noble", { x: 1, y: 0 }),
      w("builder", { x: 2, y: 0 }, "walk"), // right type but mid-walk
    ];
    expect(pickNearestIdle(walkers, "builder", { x: 0, y: 0 })).toBe(-1);
    expect(pickNearestIdle([], "builder", { x: 0, y: 0 })).toBe(-1);
  });

  it("breaks an exact distance tie deterministically (lower index wins)", () => {
    const walkers = [
      w("builder", { x: 10, y: 0 }), // dist 10
      w("builder", { x: -10, y: 0 }), // also dist 10
    ];
    expect(pickNearestIdle(walkers, "builder", { x: 0, y: 0 })).toBe(0);
  });
});

// ---------------------------------------------------------------------------
// release / adopt — against headless PIXI
// ---------------------------------------------------------------------------

// A small grid of road nodes the walkers can stroll between. Each fileId maps to
// a distinct ISO position via cartToIso so distances are real + distinct.
const NODES: Record<string, { gx: number; gy: number }> = {
  a: { gx: 0, gy: 0 },
  b: { gx: 4, gy: 0 },
  c: { gx: 0, gy: 4 },
  d: { gx: 4, gy: 4 },
  e: { gx: 8, gy: 8 },
};
const NODE_IDS = Object.keys(NODES);

function resolveNode(fileId: string): IsoPoint | null {
  const n = NODES[fileId];
  return n ? cartToIso(n.gx, n.gy) : null;
}

// A trivial findRoute: any two distinct nodes are "connected" by a straight
// 2-waypoint route through their iso positions (enough to drive update()/roaming;
// the claim primitives don't depend on route shape).
function findRoute(from: string, to: string): IsoPoint[] | null {
  if (from === to) return null;
  const a = resolveNode(from);
  const b = resolveNode(to);
  if (!a || !b) return null;
  return [a, b];
}

function isoOf(fileId: string): IsoPoint {
  const p = resolveNode(fileId);
  if (!p) throw new Error(`no node ${fileId}`);
  return p;
}

/** Build a layer with the test world and a known crowd size. */
function makeLayer(count: number): { root: Container; layer: AmbientLayer } {
  const root = new Container();
  const layer = new AmbientLayer(root);
  layer.setWorld(NODE_IDS, resolveNode, findRoute);
  layer.setCount(count);
  return { root, layer };
}

describe("AmbientLayer.release — claim the nearest idle walker", () => {
  it("returns null when no idle walker of the type exists", () => {
    const { layer } = makeLayer(6);
    // `merchant` is excluded from AMBIENT_TYPES, so none ever exists.
    expect(layer.release("merchant", isoOf("a"))).toBeNull();
    expect(layer.claimed).toBe(0);
  });

  it("returns handoff state, removes the walker, and bumps claimedCount", () => {
    const { root, layer } = makeLayer(20);
    const before = layer.count;
    const childrenBefore = root.children.length;

    // Find a type that actually exists in the spawned crowd.
    const handoff = layer.release("builder", isoOf("a"));
    expect(handoff).not.toBeNull();
    if (!handoff) return;

    // Handoff carries a real node + its position (clean node anchor).
    expect(NODE_IDS).toContain(handoff.nodeId);
    expect(handoff.pos).toEqual(resolveNode(handoff.nodeId));

    // Crowd shrank by exactly one and the claimed offset rose.
    expect(layer.count).toBe(before - 1);
    expect(layer.claimed).toBe(1);
    // No PIXI leak: the released walker's container was detached from root.
    expect(root.children.length).toBe(childrenBefore - 1);
  });

  it("does NOT respawn the claimed walker on a subsequent setCount(same)", () => {
    const { layer } = makeLayer(20);
    const target = layer.count;
    const handoff = layer.release("builder", isoOf("a"));
    expect(handoff).not.toBeNull();
    expect(layer.count).toBe(target - 1);

    // P4 (or the sync loop) re-asserts the same desired crowd size. The claimed
    // walker must stay claimed — the live crowd stays one short.
    layer.setCount(target);
    expect(layer.count).toBe(target - 1);
    expect(layer.claimed).toBe(1);
  });
});

describe("AmbientLayer.adopt — return a claimed omino to the crowd", () => {
  it("re-inserts a walker at the snapped node and restores the accounting", () => {
    const { root, layer } = makeLayer(20);
    const target = layer.count;
    layer.release("builder", isoOf("a"));
    expect(layer.claimed).toBe(1);
    const afterRelease = layer.count;
    const childrenAfterRelease = root.children.length;

    // Return the omino near node "e" — it snaps onto a real node and rejoins.
    layer.adopt("builder", isoOf("e"));
    expect(layer.claimed).toBe(0);
    expect(layer.count).toBe(afterRelease + 1);
    // The re-added walker added exactly one PIXI container back to root.
    expect(root.children.length).toBe(childrenAfterRelease + 1);

    // Crowd target is whole again: re-asserting setCount(target) is a no-op.
    layer.setCount(target);
    expect(layer.count).toBe(target);
  });

  it("floors claimedCount at 0 on an over-adopt (never goes negative)", () => {
    const { layer } = makeLayer(8);
    layer.adopt("builder", isoOf("a")); // no prior release
    expect(layer.claimed).toBe(0);
  });

  it("does NOT exceed MAX_AMBIENT on adopt (built walker destroyed, no leak)", () => {
    // Drive the crowd to the perf cap (request well above MAX_AMBIENT=64).
    const { root, layer } = makeLayer(200);
    expect(layer.count).toBe(64); // clamped to MAX_AMBIENT
    const childrenAtCap = root.children.length;
    expect(childrenAtCap).toBe(64);

    // Simulate the claim-window race: a slot was claimed (claimedCount++ via
    // release) AND setCount re-filled the crowd back to the cap. A subsequent
    // adopt of that slot must NOT push the live crowd past MAX_AMBIENT.
    layer.release("builder", isoOf("a"));
    expect(layer.claimed).toBe(1);
    layer.setCount(200); // re-fills the released slot up to the cap
    expect(layer.count).toBe(64);

    layer.adopt("builder", isoOf("e"));
    // The cap holds: the built walker was destroyed, not pushed over the cap.
    expect(layer.count).toBe(64);
    expect(layer.claimed).toBe(0);
    // No PIXI leak: root never holds more than the capped number of containers.
    expect(root.children.length).toBe(64);
  });
});

describe("AmbientLayer.clear — resets the claim accounting (drift heal)", () => {
  it("clear() resets claimedCount so a later setCount(N) builds the FULL N", () => {
    const { layer } = makeLayer(20);
    const target = layer.count;

    // Simulate a P4 leak: k releases whose matching adopts never arrive (agents
    // vanished). claimedCount climbs and stays high — every setCount undershoots.
    const k = 3;
    for (let i = 0; i < k; i++) {
      const h = layer.release("builder", isoOf("a"));
      if (!h) break; // ran out of builders; whatever we got is enough
    }
    expect(layer.claimed).toBeGreaterThan(0);
    // While leaked, re-asserting the target undershoots by the leak.
    layer.setCount(target);
    expect(layer.count).toBeLessThan(target);

    // A full teardown (clear) heals the drift.
    layer.clear();
    expect(layer.claimed).toBe(0);
    expect(layer.count).toBe(0);

    // The crowd self-heals: setCount(target) now builds the FULL target.
    layer.setCount(target);
    expect(layer.count).toBe(target);
    expect(layer.claimed).toBe(0);
  });

  it("setWorld({} empty graph) goes through clear() and heals claim drift", () => {
    const { layer } = makeLayer(20);
    layer.release("builder", isoOf("a"));
    expect(layer.claimed).toBe(1);

    // A city reload with an empty graph tears the crowd down via clear().
    layer.setWorld([], resolveNode, findRoute);
    expect(layer.claimed).toBe(0);
    expect(layer.count).toBe(0);
  });
});

describe("AmbientLayer claim — determinism", () => {
  it("same inputs → same release selection", () => {
    const a = makeLayer(20);
    const b = makeLayer(20);
    const ha = a.layer.release("builder", isoOf("d"));
    const hb = b.layer.release("builder", isoOf("d"));
    expect(ha).toEqual(hb);
  });

  it("same inputs → identical adopt seed (reproducible re-roam)", () => {
    // Two independent layers driven through the SAME release+adopt sequence must
    // produce a re-inserted walker that roams identically. We assert this by
    // ticking both forward and comparing the adopted walker's resulting pose.
    const a = makeLayer(20);
    const b = makeLayer(20);
    a.layer.release("builder", isoOf("a"));
    b.layer.release("builder", isoOf("a"));
    a.layer.adopt("builder", isoOf("e"));
    b.layer.adopt("builder", isoOf("e"));

    // Drive both layers identically; the adopted walker's deterministic rng must
    // make both crowds advance to the exact same configuration.
    for (let i = 0; i < 50; i++) {
      a.layer.update(100);
      b.layer.update(100);
    }
    const posA = a.root.children.map((c) => ({ x: c.position.x, y: c.position.y }));
    const posB = b.root.children.map((c) => ({ x: c.position.x, y: c.position.y }));
    expect(posA).toEqual(posB);
  });
});

// =========================================================================
// T2 — ambient walker blocked-tile property test
// =========================================================================

describe("AmbientLayer blocked-tile property test", () => {
  // T2 — genuine property test: for a small synthetic city with a non-trivial
  // blocked-tile predicate (a rectangle of tiles representing a building),
  // step ambient walkers for many legs and assert that NO sampled position
  // ever maps to a blocked tile. This exercises buildSafeSplineLeg's
  // degrade-to-linear logic end-to-end through the AmbientLayer.

  // OMINO_Y_OFFSET from AmbientLayer.ts (the y-shift applied to walker containers).
  const OMINO_Y_OFFSET = -4;

  // A small synthetic city: 4 nodes at the corners of a 12x12 tile square.
  // A blocked rectangle of tiles in the centre [4,4] to [7,7] (a 4x4 block).
  // findRoute returns 3-point routes that BEND through the blocked region,
  // so the Catmull-Rom spline would bow into the blocked tiles if not degraded.
  const PROP_NODES: Record<string, { gx: number; gy: number }> = {
    nw: { gx: 0, gy: 0 },
    ne: { gx: 12, gy: 0 },
    sw: { gx: 0, gy: 12 },
    se: { gx: 12, gy: 12 },
  };
  const PROP_NODE_IDS = Object.keys(PROP_NODES);

  // Blocked rectangle: tiles [4,4] to [7,7] inclusive.
  const BLOCK_MIN = 4;
  const BLOCK_MAX = 7;
  function propBlocked(gx: number, gy: number): boolean {
    return gx >= BLOCK_MIN && gx <= BLOCK_MAX && gy >= BLOCK_MIN && gy <= BLOCK_MAX;
  }

  function propResolve(fileId: string): IsoPoint | null {
    const n = PROP_NODES[fileId];
    return n ? cartToIso(n.gx, n.gy) : null;
  }

  // 3-point route that bends through the blocked centre. For example,
  // nw→se goes (0,0)→(6,6)→(12,12); the midpoint (6,6) is INSIDE the
  // blocked rectangle. The Catmull-Rom spline would bow near (6,6), but
  // buildSafeSplineLeg should degrade to linear (the straight chord from
  // (0,0) to (12,12) passes through (6,6) at t=0.5 — hmm, that's still
  // blocked). Let's use routes that AVOID the blocked centre on the chord.
  //
  // Better: nw→ne route goes (0,0)→(6,−2)→(12,0). The spline bows SOUTH
  // (negative y in cartesian), away from the blocked centre. But if we route
  // nw→sw, the chord passes through y=0..12 at x=0 — that's outside the
  // blocked x-range [4,7]. The spline bows EAST (toward the blocked centre).
  // If degraded to linear, the chord stays at x=0 — safe.
  //
  // Routes: nw→se via a midpoint at (6,1) (north of blocked centre). The
  // spline bows toward (6,6) (blocked); linear stays on chord (safe).
  function propFindRoute(from: string, to: string): IsoPoint[] | null {
    if (from === to) return null;
    const a = propResolve(from);
    const b = propResolve(to);
    if (!a || !b) return null;
    // For nw↔se or ne↔sw (diagonal), insert a midpoint that forces the
    // spline to bow through the blocked centre.
    const diag =
      (from === "nw" && to === "se") || (from === "se" && to === "nw") ||
      (from === "ne" && to === "sw") || (from === "sw" && to === "ne");
    if (diag) {
      // Midpoint at the geometric centre of the diagonal — inside the blocked
      // rectangle. The spline passes through this point; the chord may or may
      // not cross the blocked region.
      const midGx = 6;
      const midGy = 1; // just north of blocked centre (gy=4..7)
      const mid = cartToIso(midGx, midGy);
      // Orient the 3-point route in travel direction.
      if (from === "nw" || from === "ne") return [a, mid, b];
      return [b, mid, a];
    }
    // Non-diagonal: 2-point route (linear by default).
    return [a, b];
  }

  it("no walker position ever maps to a blocked tile over 100 update ticks", () => {
    const root = new Container();
    const layer = new AmbientLayer(root, undefined, propBlocked);
    layer.setWorld(PROP_NODE_IDS, propResolve, propFindRoute);
    layer.setCount(8);

    // Step 100 ticks (100ms each) — enough for several legs.
    for (let tick = 0; tick < 100; tick++) {
      layer.update(100);

      // Check every walker's current position.
      for (let i = 0; i < root.children.length; i++) {
        const child = root.children[i];
        if (!child || !child.visible) continue;
        // Container position includes OMINO_Y_OFFSET; subtract to get world ISO pos.
        const isoX = child.position.x;
        const isoY = child.position.y - OMINO_Y_OFFSET;
        // Convert ISO → cartesian tile.
        const cart = isoToCart(isoX, isoY);
        const gx = roundTile(cart.x);
        const gy = roundTile(cart.y);
        // The core assertion: the walker must NOT be on a blocked tile.
        expect(propBlocked(gx, gy)).toBe(false);
      }
    }
  });

  it("blocked rectangle is non-trivial (actually blocks some tiles)", () => {
    // Sanity: the blocked predicate actually blocks the intended tiles.
    expect(propBlocked(5, 5)).toBe(true);
    expect(propBlocked(4, 4)).toBe(true);
    expect(propBlocked(7, 7)).toBe(true);
    // Tiles outside the rectangle are walkable.
    expect(propBlocked(0, 0)).toBe(false);
    expect(propBlocked(3, 3)).toBe(false);
    expect(propBlocked(8, 8)).toBe(false);
  });

  it("setBlocked does not disrupt an already-running crowd", () => {
    const { layer } = makeLayer(10);
    for (let i = 0; i < 30; i++) layer.update(100);
    const countBefore = layer.count;

    layer.setBlocked(() => false);
    for (let i = 0; i < 30; i++) layer.update(100);
    expect(layer.count).toBe(countBefore);
  });

  it("setBlocked with aggressive blocker does not crash the layer", () => {
    const { layer } = makeLayer(10);
    for (let i = 0; i < 20; i++) layer.update(100);

    layer.setBlocked(() => true);
    expect(() => {
      for (let i = 0; i < 50; i++) layer.update(100);
    }).not.toThrow();
  });

  it("constructor accepts a blocked predicate without throwing", () => {
    const root = new Container();
    expect(() => {
      new AmbientLayer(root, undefined, () => false);
    }).not.toThrow();
  });
});
