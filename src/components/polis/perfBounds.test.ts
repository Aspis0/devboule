import { describe, it, expect, vi, beforeEach } from "vitest";

// Polis-P6 — PERF BOUNDS + LOD contract tests (headless PIXI v8). Proves:
//   - LOD halt: setLodVisible(false) hides AND stops animating every walker type
//     (real agents, external Censor firefighter, subagents, ambient crowd) — a
//     step/update tick does NO figure redraw while LOD-hidden.
//   - No per-frame pathfinding: a step/update tick with placed agents/subs/an
//     external omino (and a roaming crowd) calls the route function ZERO times.
//
// `drawCitizen` is mocked to a spy so a redraw is observable; `findRoute` is a spy
// the layers receive so per-frame pathfinding is observable. Both reset between
// tests so only the step/update window is asserted.

const drawCitizenSpy = vi.fn();
vi.mock("./kitcd/people", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./kitcd/people")>();
  return {
    ...actual,
    // Spy wrapper that still no-ops the drawing (headless: we only count calls).
    drawCitizen: (...args: unknown[]) => {
      drawCitizenSpy(...args);
    },
  };
});

// Imports AFTER the mock so the layers pick up the spied drawCitizen.
const { Container, Rectangle } = await import("pixi.js");
const { AgentLayer } = await import("./AgentLayer");
const { AmbientLayer } = await import("./AmbientLayer");
const { cartToIso } = await import("./iso");
import type { IsoPoint } from "./iso";
import type { Agent } from "../../types/city";
import type { PossessionDecision } from "./possession";

// ---------------------------------------------------------------------------
// Test world
// ---------------------------------------------------------------------------

const NODES: Record<string, IsoPoint> = {
  f1: cartToIso(0, 0),
  f2: cartToIso(6, 2),
  f3: cartToIso(3, 6),
  f4: cartToIso(8, 8),
};
const NODE_IDS = Object.keys(NODES);

function resolveNode(fileId: string): IsoPoint | null {
  return NODES[fileId] ?? null;
}

/** A route spy: any two distinct resolvable nodes are "connected" by a straight
 *  2-waypoint route. Wrapped so the call count is observable. */
const findRouteSpy = vi.fn((from: string, to: string): IsoPoint[] | null => {
  if (from === to) return null;
  const a = resolveNode(from);
  const b = resolveNode(to);
  if (!a || !b) return null;
  return [a, b];
});

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

/** Populate an AgentLayer with a placed agent, a subagent and an external Censor
 *  firefighter (all the walker types AgentLayer owns). */
function populatedAgentLayer(): { root: InstanceType<typeof Container>; layer: InstanceType<typeof AgentLayer> } {
  const root = new Container();
  const layer = new AgentLayer(root);
  const decisions: PossessionDecision[] = [
    {
      kind: "createFresh",
      agentId: "a1",
      agent: mkAgent({ currentFileId: "f1" }),
      targetFileId: "f1",
      targetIso: NODES.f1,
    },
    {
      kind: "spawnSub",
      subId: "a1::sub::coder::0",
      parentAgentId: "a1",
      role: "coder",
      figure: "builder",
      pos: { x: NODES.f1.x + 20, y: NODES.f1.y },
    },
  ];
  layer.applyDecisions(decisions, findRouteSpy);
  // External Censor firefighter walking from a node to a building (uses findRoute
  // on CREATE — that's allowed; the per-frame step/update must NOT).
  layer.createExternalClaimed(
    "censor:p",
    "firefighter",
    NODES.f3,
    "f3",
    "f4",
    NODES.f4,
    findRouteSpy,
  );
  return { root, layer };
}

function populatedAmbientLayer(): { root: InstanceType<typeof Container>; layer: InstanceType<typeof AmbientLayer> } {
  const root = new Container();
  const layer = new AmbientLayer(root);
  layer.setWorld(NODE_IDS, resolveNode, findRouteSpy);
  layer.setCount(6);
  return { root, layer };
}

beforeEach(() => {
  drawCitizenSpy.mockClear();
  findRouteSpy.mockClear();
});

// ---------------------------------------------------------------------------
// LOD halt — hidden walkers do no per-frame redraw
// ---------------------------------------------------------------------------

describe("Polis-P6 LOD halt — AgentLayer", () => {
  it("setLodVisible(false) → step()/update() redraw NO figures (agents, subs, external)", () => {
    const { layer } = populatedAgentLayer();
    // Sanity: while VISIBLE a step redraws (agent + sub + external = redraws > 0).
    drawCitizenSpy.mockClear();
    layer.step(2);
    expect(drawCitizenSpy.mock.calls.length).toBeGreaterThan(0);

    // Hide everything: a step + update must redraw NOTHING.
    layer.setLodVisible(false);
    drawCitizenSpy.mockClear();
    for (let f = 0; f < 6; f++) {
      layer.step(f);
      layer.update(33);
    }
    expect(drawCitizenSpy).not.toHaveBeenCalled();
  });

  it("re-showing (setLodVisible(true)) resumes the figure redraw", () => {
    const { layer } = populatedAgentLayer();
    layer.setLodVisible(false);
    drawCitizenSpy.mockClear();
    layer.step(0);
    expect(drawCitizenSpy).not.toHaveBeenCalled();

    layer.setLodVisible(true);
    drawCitizenSpy.mockClear();
    layer.step(0);
    expect(drawCitizenSpy.mock.calls.length).toBeGreaterThan(0);
  });
});

describe("Polis-P6 LOD halt — AmbientLayer", () => {
  it("setLodVisible(false) → step()/update() redraw NO citizens and advance no movement", () => {
    const { layer } = populatedAmbientLayer();
    // Sanity: visible step redraws the crowd.
    drawCitizenSpy.mockClear();
    layer.step(2);
    expect(drawCitizenSpy.mock.calls.length).toBeGreaterThan(0);

    layer.setLodVisible(false);
    drawCitizenSpy.mockClear();
    for (let f = 0; f < 6; f++) {
      layer.step(f);
      layer.update(100);
    }
    expect(drawCitizenSpy).not.toHaveBeenCalled();
    // And the frozen crowd advanced no movement → no pathfinding while hidden.
    expect(findRouteSpy).not.toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------------------
// No per-frame pathfinding — step()/update() never call findRoute
// ---------------------------------------------------------------------------

describe("Polis-P6 no per-frame findRoute — AgentLayer", () => {
  it("step()/update() with placed agents, subs and an external omino call findRoute 0 times", () => {
    const { layer } = populatedAgentLayer();
    findRouteSpy.mockClear(); // ignore CREATE-time routing
    for (let f = 0; f < 30; f++) {
      layer.step(f);
      layer.update(33);
    }
    expect(findRouteSpy).not.toHaveBeenCalled();
  });
});

describe("Polis-P6 no per-frame findRoute — AmbientLayer", () => {
  it("a roaming crowd's step()/update() ticks may re-pick targets but never per-frame floods routing", () => {
    const { layer } = populatedAmbientLayer();
    // A roaming crowd DOES call findRoute when a walker picks a new destination
    // (on a stop boundary — NOT every frame). To assert the per-frame contract we
    // count routing across many frames and require it to be far below one-per-
    // walker-per-frame: i.e. routing is event-driven (target picks), not per-frame.
    findRouteSpy.mockClear();
    const frames = 50;
    for (let f = 0; f < frames; f++) {
      layer.step(f);
      layer.update(16); // ~60fps frames; walkers mostly mid-walk/idle, few re-picks
    }
    // Per-frame routing of 6 walkers over 50 frames would be ~300 calls. Target
    // re-picks happen only on arrival/idle-expiry, so the real count is a small
    // fraction. Lock it well under the per-frame ceiling.
    expect(findRouteSpy.mock.calls.length).toBeLessThan(frames);
  });
});

// ---------------------------------------------------------------------------
// #9 — viewport cull: step(frame, view) skips the redraw for OFF-SCREEN walkers
// (subagents in AgentLayer, the ambient crowd in AmbientLayer) and re-draws them
// when they scroll back into view. Movement is uncoupled from the redraw.
// ---------------------------------------------------------------------------

describe("Polis-P6 viewport cull (#9) — AgentLayer subagents", () => {
  it("redraws a sub that is IN view and skips one that is OUT of view (re-shows on re-entry)", () => {
    // A layer with ONLY a subagent (a distinctive "noble" figure so the spy filter
    // can't collide with a real agent's figure) at a known position.
    const root = new Container();
    const layer = new AgentLayer(root);
    const subPos = { x: NODES.f1.x + 20, y: NODES.f1.y };
    layer.applyDecisions(
      [
        {
          kind: "spawnSub",
          subId: "a1::sub::reviewer::0",
          parentAgentId: "a1",
          role: "reviewer",
          figure: "noble",
          pos: subPos,
        },
      ],
      findRouteSpy,
    );
    const inView = new Rectangle(subPos.x - 100, subPos.y - 100, 200, 200);
    const offView = new Rectangle(subPos.x + 5000, subPos.y + 5000, 200, 200);

    // OUT of view → the sub figure is NOT redrawn.
    drawCitizenSpy.mockClear();
    layer.step(2, offView);
    expect(drawCitizenSpy.mock.calls.filter((c) => c[1] === "noble")).toHaveLength(0);

    // Back IN view → the sub IS redrawn again (re-entry never leaves it blank).
    drawCitizenSpy.mockClear();
    layer.step(3, inView);
    expect(drawCitizenSpy.mock.calls.filter((c) => c[1] === "noble").length).toBeGreaterThan(0);
  });
});

describe("Polis-P6 viewport cull (#9) — AmbientLayer crowd", () => {
  it("an entirely off-screen view redraws NO walkers; an enclosing view redraws them", () => {
    const { layer } = populatedAmbientLayer();
    // A huge view that encloses the whole test world → every visible walker redraws.
    const allView = new Rectangle(-10000, -10000, 20000, 20000);
    // A far-away view that contains none of the nodes → no walker redraw.
    const offView = new Rectangle(100000, 100000, 100, 100);

    drawCitizenSpy.mockClear();
    layer.step(2, offView);
    expect(drawCitizenSpy).not.toHaveBeenCalled();

    drawCitizenSpy.mockClear();
    layer.step(3, allView);
    expect(drawCitizenSpy.mock.calls.length).toBeGreaterThan(0);
  });

  it("without a view arg the crowd still redraws (headless / no-cull path)", () => {
    const { layer } = populatedAmbientLayer();
    drawCitizenSpy.mockClear();
    layer.step(2);
    expect(drawCitizenSpy.mock.calls.length).toBeGreaterThan(0);
  });
});
