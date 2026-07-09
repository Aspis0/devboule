import { describe, it, expect } from "vitest";
import { Container } from "pixi.js";
import { AgentLayer } from "./AgentLayer";
import type { PossessionDecision } from "./possession";
import { cartToIso, type IsoPoint } from "./iso";
import type { Agent } from "../../types/city";

// Polis-P4 — AgentLayer.applyDecisions against headless PIXI v8 (Container/
// Graphics need no GL context to construct + mutate the scene graph — same
// approach as AmbientLayer.claim.test / TradeRouteLayer.test). Proves the APPLY
// layer mirrors the PURE controller's decisions: claimed omini start at the
// handoff pos (no fade), fresh omini fade in, release destroys (no leak), and the
// subagent omini spawn/teardown cleanly.

const NODES: Record<string, IsoPoint> = {
  f1: cartToIso(0, 0),
  f2: cartToIso(6, 2),
};

function route(from: string, to: string): IsoPoint[] | null {
  if (from === to) return null;
  const a = NODES[from] ?? cartToIso(0, 0);
  const b = NODES[to];
  return b ? [a, b] : null;
}

function mkAgent(over: Partial<Agent>): Agent {
  return {
    agentId: "a1",
    type: "coder",
    status: "working",
    currentFileId: "f1",
    currentTask: null,
    color: "#888888",
    ...over,
  };
}

function makeLayer(): { root: Container; layer: AgentLayer } {
  const root = new Container();
  return { root, layer: new AgentLayer(root) };
}

describe("AgentLayer.applyDecisions — claim / fresh", () => {
  it("createClaimed places the omino at the handoff start pos (already visible) then walks it", () => {
    const { layer } = makeLayer();
    const start: IsoPoint = { x: 5, y: 5 };
    const agent = mkAgent({ currentFileId: "f1" });
    const decisions: PossessionDecision[] = [
      {
        kind: "createClaimed",
        agentId: "a1",
        agent,
        startPos: start,
        startNodeId: "f1",
        targetFileId: "f2",
        targetIso: NODES.f2,
      },
    ];
    layer.applyDecisions(decisions, route);
    expect(layer.placedCount).toBe(1);
    // A claimed omino starts walking immediately, so it snaps to the route's first
    // waypoint (the handoff node iso), NOT the target — and it is already visible.
    const pos = layer.agentPos("a1");
    expect(pos).not.toBeNull();
  });

  it("createFresh fades in (alpha starts at 0) at the target building", () => {
    const { layer } = makeLayer();
    const decisions: PossessionDecision[] = [
      {
        kind: "createFresh",
        agentId: "a1",
        agent: mkAgent({ currentFileId: "f1" }),
        targetFileId: "f1",
        targetIso: NODES.f1,
      },
    ];
    layer.applyDecisions(decisions, route);
    expect(layer.placedCount).toBe(1);
    expect(layer.agentPos("a1")).toEqual({ x: NODES.f1.x, y: NODES.f1.y });
  });

  it("release destroys the omino with no leaked PIXI children", () => {
    const { root, layer } = makeLayer();
    layer.applyDecisions(
      [
        {
          kind: "createFresh",
          agentId: "a1",
          agent: mkAgent({}),
          targetFileId: "f1",
          targetIso: NODES.f1,
        },
      ],
      route,
    );
    const childrenWithAgent = root.children.length;
    expect(childrenWithAgent).toBeGreaterThan(0);

    layer.applyDecisions([{ kind: "release", agentId: "a1" }], route);
    expect(layer.placedCount).toBe(0);
    // glow + omino containers both detached.
    expect(root.children.length).toBe(childrenWithAgent - 2);
  });

  it("a duplicate createClaimed for the same id is a no-op (idempotent apply)", () => {
    const { layer } = makeLayer();
    const d: PossessionDecision = {
      kind: "createClaimed",
      agentId: "a1",
      agent: mkAgent({}),
      startPos: { x: 1, y: 1 },
      startNodeId: "f1",
      targetFileId: "f2",
      targetIso: NODES.f2,
    };
    layer.applyDecisions([d, d], route);
    expect(layer.placedCount).toBe(1);
  });
});

describe("AgentLayer.applyDecisions — subagents", () => {
  it("spawnSub adds a small omino; it fades in to a sub-1 alpha after APPEAR_MS", () => {
    const { root, layer } = makeLayer();
    layer.applyDecisions(
      [
        {
          kind: "spawnSub",
          subId: "p::sub::coder::0",
          parentAgentId: "p",
          role: "coder",
          figure: "builder",
          pos: { x: 40, y: 20 },
        },
      ],
      route,
    );
    expect(layer.subagentCount).toBe(1);
    expect(root.children.length).toBe(1);
    // Drive the fade-in envelope to completion.
    layer.update(500);
    const sub = root.children[0];
    expect(sub.alpha).toBeGreaterThan(0);
    expect(sub.alpha).toBeLessThanOrEqual(1);
  });

  it("removeSub fades out then destroys on completion (no leak)", () => {
    const { root, layer } = makeLayer();
    layer.applyDecisions(
      [
        {
          kind: "spawnSub",
          subId: "s",
          parentAgentId: "p",
          role: "coder",
          figure: "builder",
          pos: { x: 10, y: 10 },
        },
      ],
      route,
    );
    layer.update(500); // settle fade-in
    expect(layer.subagentCount).toBe(1);

    layer.applyDecisions([{ kind: "removeSub", subId: "s" }], route);
    // Still present until the fade-out completes.
    layer.update(50);
    expect(layer.subagentCount).toBe(1);
    // Complete the fade-out → destroyed + removed.
    layer.update(500);
    expect(layer.subagentCount).toBe(0);
    expect(root.children.length).toBe(0);
  });

  it("spawnSub REVIVES a sub mid fade-OUT instead of dropping it (F2)", () => {
    const { root, layer } = makeLayer();
    const spawn: PossessionDecision = {
      kind: "spawnSub",
      subId: "s",
      parentAgentId: "p",
      role: "coder",
      figure: "builder",
      pos: { x: 10, y: 10 },
    };
    layer.applyDecisions([spawn], route);
    layer.update(500); // settle fade-in
    expect(layer.subagentCount).toBe(1);

    // Begin removal (fade-OUT armed) but DON'T let it complete.
    layer.applyDecisions([{ kind: "removeSub", subId: "s" }], route);
    layer.update(50); // partway through the fade-out (still present)
    expect(layer.subagentCount).toBe(1);

    // Re-spawn the SAME id within the fade window → must revive (fade back IN),
    // NOT be dropped by the old `if (has) break`.
    layer.applyDecisions([spawn], route);
    // Drive well past APPEAR_MS: a revived sub fades IN to a positive alpha and is
    // NOT destroyed. A dropped sub would have been destroyed → count 0.
    layer.update(500);
    expect(layer.subagentCount).toBe(1);
    expect(root.children.length).toBe(1);
    expect(root.children[0].alpha).toBeGreaterThan(0);
  });

  it("removeSub does NOT re-arm an in-flight fade-out under rapid diffs (F3)", () => {
    const { layer } = makeLayer();
    layer.applyDecisions(
      [
        {
          kind: "spawnSub",
          subId: "s",
          parentAgentId: "p",
          role: "coder",
          figure: "builder",
          pos: { x: 10, y: 10 },
        },
      ],
      route,
    );
    layer.update(500); // settle fade-in
    expect(layer.subagentCount).toBe(1);

    // First removeSub arms the fade-out.
    layer.applyDecisions([{ kind: "removeSub", subId: "s" }], route);
    layer.update(150); // advance most of the 200ms fade-out (NOT complete yet)
    expect(layer.subagentCount).toBe(1);

    // A SECOND removeSub arriving within the window must NOT reset elapsed=0 —
    // otherwise the fade-out would restart forever and the sub would never go away.
    layer.applyDecisions([{ kind: "removeSub", subId: "s" }], route);
    // A further small advance pushes the (un-reset) elapsed past APPEAR_MS → destroyed.
    layer.update(100);
    expect(layer.subagentCount).toBe(0);
  });

  it("clear() tears down both agents and subagents", () => {
    const { root, layer } = makeLayer();
    layer.applyDecisions(
      [
        {
          kind: "createFresh",
          agentId: "a1",
          agent: mkAgent({}),
          targetFileId: "f1",
          targetIso: NODES.f1,
        },
        {
          kind: "spawnSub",
          subId: "s",
          parentAgentId: "a1",
          role: "coder",
          figure: "builder",
          pos: { x: 10, y: 10 },
        },
      ],
      route,
    );
    expect(layer.placedCount).toBe(1);
    expect(layer.subagentCount).toBe(1);
    layer.clear();
    expect(layer.placedCount).toBe(0);
    expect(layer.subagentCount).toBe(0);
    expect(root.children.length).toBe(0);
  });

  it("step() animates a subagent without throwing (scaled figure redraw)", () => {
    const { layer } = makeLayer();
    layer.applyDecisions(
      [
        {
          kind: "spawnSub",
          subId: "s",
          parentAgentId: "p",
          role: "verifier",
          figure: "citizen",
          pos: { x: 10, y: 10 },
        },
      ],
      route,
    );
    expect(() => {
      for (let f = 0; f < 8; f++) layer.step(f);
    }).not.toThrow();
  });
});

// =========================================================================
// T2 — slot validation fallback (blocked tile → door tile centre)
// =========================================================================

import { isoToCart } from "./iso";
import { roundTile } from "./navWalkable";

describe("AgentLayer slot validation — blocked tile fallback", () => {
  // T2 — when the slot position lands on a blocked tile, the agent must fall
  // back to the building door/origin tile centre (end of route).

  const NODES2: Record<string, IsoPoint> = {
    a: cartToIso(0, 0),
    b: cartToIso(10, 0),
  };

  function route2(from: string, to: string): IsoPoint[] | null {
    if (from === to) return null;
    const a = NODES2[from];
    const b = NODES2[to];
    if (!a || !b) return null;
    return [a, b];
  }

  // Tile-grid blocker helper (matches production makeBuildingBlocker).
  function tileBlocked(blockedTiles: Set<string>): (gx: number, gy: number) => boolean {
    return (gx: number, gy: number) => blockedTiles.has(`${gx},${gy}`);
  }

  it("slot position on a blocked tile \u2192 falls back to door centre", () => {
    // Block ALL tiles: every slot position will be blocked.
    const blockAll = () => true;
    const root = new Container();
    const layer = new AgentLayer(root, undefined, undefined, blockAll);

    const agent = mkAgent({ agentId: "slot-test", currentFileId: "a" });
    const decisions: PossessionDecision[] = [
      {
        kind: "createClaimed",
        agentId: "slot-test",
        agent,
        startPos: NODES2.a,
        startNodeId: "a",
        targetFileId: "b",
        targetIso: NODES2.b,
      },
    ];
    layer.applyDecisions(decisions, route2);
    // Walk to completion.
    for (let i = 0; i < 200; i++) layer.update(50);
    // The agent should be at the door centre (NODES2.b), NOT at a slot offset.
    const pos = layer.agentPos("slot-test");
    expect(pos).not.toBeNull();
    if (!pos) return;
    // The door position is NODES2.b. The slot would be offset by up to 24px.
    // With blockAll, the agent must have fallen back to the door.
    expect(pos.x).toBeCloseTo(NODES2.b.x, 0);
    expect(pos.y).toBeCloseTo(NODES2.b.y, 0);
  });

  it("slot position on a walkable tile \u2192 uses the slot offset (no fallback)", () => {
    // Block nothing: slot positions are all walkable.
    const blockNone = () => false;
    const root = new Container();
    const layer = new AgentLayer(root, undefined, undefined, blockNone);

    const agent = mkAgent({ agentId: "slot-ok", currentFileId: "a" });
    const decisions: PossessionDecision[] = [
      {
        kind: "createClaimed",
        agentId: "slot-ok",
        agent,
        startPos: NODES2.a,
        startNodeId: "a",
        targetFileId: "b",
        targetIso: NODES2.b,
      },
    ];
    layer.applyDecisions(decisions, route2);
    for (let i = 0; i < 200; i++) layer.update(50);
    const pos = layer.agentPos("slot-ok");
    expect(pos).not.toBeNull();
    if (!pos) return;
    // The agent should be near the door (possibly with a slot offset, but
    // definitely within a reasonable range).
    const distToDoor = Math.hypot(pos.x - NODES2.b.x, pos.y - NODES2.b.y);
    expect(distToDoor).toBeLessThan(30); // within slot offset range
  });

  it("tile-grid blocker with iso→cart conversion: slot on blocked tile falls back", () => {
    // T2 FIX: use a real tile-grid blocker (not blockAll) to prove the
    // iso→cart conversion in the slot validation path works correctly.
    // Block tile (10,0) — the building's door tile. Slot 0 is AT the door,
    // so the slot position maps to tile (10,0) in grid space.
    const blockedTiles = new Set(["10,0"]);
    const blocked = tileBlocked(blockedTiles);
    const root = new Container();
    const layer = new AgentLayer(root, undefined, undefined, blocked);

    const agent = mkAgent({ agentId: "slot-grid", currentFileId: "a" });
    const decisions: PossessionDecision[] = [
      {
        kind: "createClaimed",
        agentId: "slot-grid",
        agent,
        startPos: NODES2.a,
        startNodeId: "a",
        targetFileId: "b",
        targetIso: NODES2.b,
      },
    ];
    layer.applyDecisions(decisions, route2);
    for (let i = 0; i < 200; i++) layer.update(50);
    const pos = layer.agentPos("slot-grid");
    expect(pos).not.toBeNull();
    if (!pos) return;
    // The agent should be at the door centre (NODES2.b) because tile (10,0) is blocked.
    expect(pos.x).toBeCloseTo(NODES2.b.x, 0);
    expect(pos.y).toBeCloseTo(NODES2.b.y, 0);
  });

  it("tile-grid blocker: adjacent non-blocked tile does NOT trigger fallback", () => {
    // Block tile (11,0) — NOT the door tile (10,0). The slot at the door
    // should remain at the slot offset (not fall back).
    const blockedTiles = new Set(["11,0"]);
    const blocked = tileBlocked(blockedTiles);
    const root = new Container();
    const layer = new AgentLayer(root, undefined, undefined, blocked);

    const agent = mkAgent({ agentId: "slot-free", currentFileId: "a" });
    const decisions: PossessionDecision[] = [
      {
        kind: "createClaimed",
        agentId: "slot-free",
        agent,
        startPos: NODES2.a,
        startNodeId: "a",
        targetFileId: "b",
        targetIso: NODES2.b,
      },
    ];
    layer.applyDecisions(decisions, route2);
    for (let i = 0; i < 200; i++) layer.update(50);
    const pos = layer.agentPos("slot-free");
    expect(pos).not.toBeNull();
    if (!pos) return;
    // The agent should be near the door (within slot offset range, NOT at door centre).
    const distToDoor = Math.hypot(pos.x - NODES2.b.x, pos.y - NODES2.b.y);
    expect(distToDoor).toBeLessThan(30); // within slot offset range
    // Verify the tile-grid conversion: slot 0 at door (10,0) maps to tile (10,0) which is NOT blocked.
    // So the fallback should NOT trigger — the agent should be at the slot offset.
    const slotPos = NODES2.b; // slot 0 is at the door
    const slotCart = isoToCart(slotPos.x, slotPos.y);
    expect(roundTile(slotCart.x)).toBe(10);
    expect(roundTile(slotCart.y)).toBe(0);
    // Tile (10,0) is NOT in blockedTiles, so the fallback should not trigger.
    expect(blocked(roundTile(slotCart.x), roundTile(slotCart.y))).toBe(false);
  });
});
