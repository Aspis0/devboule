import { describe, it, expect } from "vitest";
import { Container } from "pixi.js";
import { AgentLayer } from "./AgentLayer";
import { CENSOR_FIGURE } from "./censorPresence";
import { cartToIso, type IsoPoint } from "./iso";

// Polis-P5 — the AgentLayer EXTERNAL omino API (the Censor firefighter), tested
// against headless PIXI v8 (Container/Graphics construct + mutate with no GL — the
// same approach as AgentLayer.possession.test). Proves the firefighter is driven
// directly (create/walk/extinguish/destroy), is NEVER counted as an agent
// (`placedCount` excludes it → never in the roster), and is torn down cleanly.

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

function makeLayer(): { root: Container; layer: AgentLayer } {
  const root = new Container();
  return { root, layer: new AgentLayer(root) };
}

const ID = "censor:p1";

describe("AgentLayer — external Censor firefighter", () => {
  it("createExternalClaimed places ONE external omino, NOT an agent", () => {
    const { layer } = makeLayer();
    layer.createExternalClaimed(
      ID,
      CENSOR_FIGURE,
      { x: 5, y: 5 },
      "f1",
      "f2",
      NODES.f2,
      route,
    );
    expect(layer.externalCount).toBe(1);
    // CRITICAL: it must NEVER count as an agent (so it never shows in the roster
    // and is never part of the agent diff / city.agents).
    expect(layer.placedCount).toBe(0);
    expect(layer.subagentCount).toBe(0);
    expect(layer.externalPos(ID)).not.toBeNull();
    // It is not addressable as an agent.
    expect(layer.agentPos(ID)).toBeNull();
  });

  it("createExternalFresh places one external omino at the target", () => {
    const { layer } = makeLayer();
    layer.createExternalFresh(ID, CENSOR_FIGURE, "f2", NODES.f2);
    expect(layer.externalCount).toBe(1);
    expect(layer.placedCount).toBe(0);
  });

  it("a duplicate create id is ignored (idempotent — one omino per id)", () => {
    const { layer } = makeLayer();
    layer.createExternalFresh(ID, CENSOR_FIGURE, "f2", NODES.f2);
    layer.createExternalFresh(ID, CENSOR_FIGURE, "f1", NODES.f1);
    expect(layer.externalCount).toBe(1);
  });

  it("walkExternal moves the existing omino (no new omino)", () => {
    const { layer } = makeLayer();
    layer.createExternalFresh(ID, CENSOR_FIGURE, "f1", NODES.f1);
    layer.walkExternal(ID, "f2", NODES.f2, route);
    expect(layer.externalCount).toBe(1);
    // It survived the walk (still addressable) and remains a non-agent.
    expect(layer.externalPos(ID)).not.toBeNull();
    expect(layer.placedCount).toBe(0);
  });

  it("setExternalExtinguishing + destroyExternal are no-throw and leak-free", () => {
    const { layer } = makeLayer();
    layer.createExternalFresh(ID, CENSOR_FIGURE, "f2", NODES.f2);
    layer.setExternalExtinguishing(ID, true);
    layer.setExternalExtinguishing(ID, false);
    layer.destroyExternal(ID);
    expect(layer.externalCount).toBe(0);
    expect(layer.externalPos(ID)).toBeNull();
    // A second destroy is a harmless no-op.
    layer.destroyExternal(ID);
    expect(layer.externalCount).toBe(0);
  });

  it("operations on an unknown id are safe no-ops", () => {
    const { layer } = makeLayer();
    layer.walkExternal("nope", "f2", NODES.f2, route);
    layer.setExternalExtinguishing("nope", true);
    layer.destroyExternal("nope");
    expect(layer.externalCount).toBe(0);
  });

  it("clear() tears down external omini (no stray firefighter after a reload)", () => {
    const { layer } = makeLayer();
    layer.createExternalFresh(ID, CENSOR_FIGURE, "f2", NODES.f2);
    layer.createExternalFresh("censor:p2", CENSOR_FIGURE, "f1", NODES.f1);
    expect(layer.externalCount).toBe(2);
    layer.clear();
    expect(layer.externalCount).toBe(0);
  });

  it("step + update run with an external omino present (no crash, no marker ref)", () => {
    const { layer } = makeLayer();
    layer.createExternalFresh(ID, CENSOR_FIGURE, "f2", NODES.f2);
    layer.setLodVisible(true);
    // The external omino has NO marker; stepping must not dereference one.
    expect(() => {
      for (let f = 0; f < 8; f++) layer.step(f);
      layer.update(16);
    }).not.toThrow();
    expect(layer.externalCount).toBe(1);
  });
});
