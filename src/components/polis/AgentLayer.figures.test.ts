import { describe, it, expect } from "vitest";
import { Graphics } from "pixi.js";
import {
  figureForAgent,
  figureForRole,
  SUBAGENT_FIGURE_SCALE,
  FIGURE_SCALE,
  subagentFigureScale,
} from "./AgentLayer";
import { drawCitizen } from "./kitcd/people";
import type { Agent } from "../../types/city";

// Polis-P2 — entity→figure mapping (mapping + scale-support + vocabulary only;
// no claim/possession/movement). These run headlessly: PIXI v8 Graphics is a
// plain scene-graph object, so figure drawing needs no WebGL renderer.

function mkAgent(over: Partial<Agent>): Agent {
  return {
    agentId: "a1",
    type: "coder",
    status: "idle",
    currentFileId: "f1",
    currentTask: null,
    color: "#888888",
    ...over,
  };
}

describe("figureForAgent — agent type/parent → kit figure", () => {
  it("maps coder → builder", () => {
    expect(figureForAgent(mkAgent({ type: "coder" }))).toBe("builder");
  });

  it("maps orchestrator → noble", () => {
    expect(figureForAgent(mkAgent({ type: "orchestrator" }))).toBe("noble");
  });

  it("maps verifier → citizen", () => {
    expect(figureForAgent(mkAgent({ type: "verifier" }))).toBe("citizen");
  });

  it("maps an unknown type → citizen (graceful default)", () => {
    expect(figureForAgent(mkAgent({ type: "augur" }))).toBe("citizen");
    expect(figureForAgent(mkAgent({ type: "whatever" }))).toBe("citizen");
  });

  it("mini-coder precedence: parentAgentId set → watercarrier, beating type", () => {
    // A coder spawned by a parent coder reads as a mini-coder (watercarrier),
    // NOT the plain coder builder.
    expect(
      figureForAgent(mkAgent({ type: "coder", parentAgentId: "parent" })),
    ).toBe("watercarrier");
    // Precedence holds regardless of the underlying type.
    expect(
      figureForAgent(mkAgent({ type: "orchestrator", parentAgentId: "p" })),
    ).toBe("watercarrier");
    expect(
      figureForAgent(mkAgent({ type: "verifier", parentAgentId: "p" })),
    ).toBe("watercarrier");
  });
});

describe("figureForRole — subagent role → kit figure", () => {
  it("maps coder → builder", () => {
    expect(figureForRole("coder")).toBe("builder");
  });

  it("maps verifier → citizen", () => {
    expect(figureForRole("verifier")).toBe("citizen");
  });

  it("maps an unknown role → citizen (graceful default)", () => {
    expect(figureForRole("anything")).toBe("citizen");
    expect(figureForRole("")).toBe("citizen");
  });

  it("non-subagent roles fall to the citizen default", () => {
    // Subagent roles are only coder/verifier; everything else (noble,
    // watercarrier, …) must land on the citizen default. Locks that path.
    expect(figureForRole("noble")).toBe("citizen");
    expect(figureForRole("watercarrier")).toBe("citizen");
  });
});

describe("subagent figure scale (for P4 scaled-down omini)", () => {
  it("SUBAGENT_FIGURE_SCALE is a sub-1 reduction", () => {
    expect(SUBAGENT_FIGURE_SCALE).toBeGreaterThan(0);
    expect(SUBAGENT_FIGURE_SCALE).toBeLessThan(1);
  });

  it("subagentFigureScale() is the base figure scale times the subagent factor", () => {
    expect(subagentFigureScale()).toBeCloseTo(FIGURE_SCALE * SUBAGENT_FIGURE_SCALE);
    expect(subagentFigureScale()).toBeLessThan(FIGURE_SCALE);
  });

  it("a scaled figure renders without error and applies the scale to the node", () => {
    const g = new Graphics();
    g.scale.set(subagentFigureScale());
    drawCitizen(g, figureForRole("coder"), {
      moving: false,
      phase: 0,
      actionPhase: 0,
    });
    expect(g.scale.x).toBeCloseTo(subagentFigureScale());
    expect(g.scale.y).toBeCloseTo(subagentFigureScale());
  });
});
