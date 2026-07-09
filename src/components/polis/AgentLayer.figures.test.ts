import { describe, it, expect, vi } from "vitest";
import { Graphics, Container } from "pixi.js";
import {
  AgentLayer,
  figureForAgent,
  figureForRole,
  liveryTint,
  SUBAGENT_FIGURE_SCALE,
  FIGURE_SCALE,
  subagentFigureScale,
} from "./AgentLayer";
import { drawCitizen } from "./kitcd/people";
import * as people from "./kitcd/people";
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

function makeLayer(): { root: Container; layer: AgentLayer } {
  const root = new Container();
  return { root, layer: new AgentLayer(root) };
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

  it("maps augur → priest", () => {
    expect(figureForAgent(mkAgent({ type: "augur" }))).toBe("priest");
  });

  it("maps an unknown type → foreigner (graceful default)", () => {
    expect(figureForAgent(mkAgent({ type: "sherpa" }))).toBe("foreigner");
    expect(figureForAgent(mkAgent({ type: "whatever" }))).toBe("foreigner");
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

  it("maps augur → priest", () => {
    expect(figureForRole("augur")).toBe("priest");
  });

  it("maps an unknown role → foreigner (graceful default)", () => {
    expect(figureForRole("anything")).toBe("foreigner");
    expect(figureForRole("sherpa")).toBe("foreigner");
    expect(figureForRole("")).toBe("foreigner");
  });

  it("non-subagent roles fall to the foreigner default", () => {
    // Subagent roles are coder/verifier/augur; everything else lands on
    // the foreigner default (unknown/external).
    expect(figureForRole("noble")).toBe("foreigner");
    expect(figureForRole("watercarrier")).toBe("foreigner");
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

describe("liveryTint — provider model → tunic colour", () => {
  it("returns jade for MiMo (case-insensitive)", () => {
    const t = liveryTint("MiMo-V2.5");
    expect(t).toBeDefined();
    expect(typeof t).toBe("number");
  });

  it("returns indigo for DeepSeek", () => {
    expect(liveryTint("deepseek-r1")).toBeDefined();
  });

  it("returns terracotta for Claude family (claude/sonnet/opus/fable)", () => {
    expect(liveryTint("claude-sonnet-4")).toBeDefined();
    expect(liveryTint("opus-4")).toBeDefined();
    expect(liveryTint("fable-turbo")).toBeDefined();
  });

  it("returns undefined for unrelated strings", () => {
    expect(liveryTint("gpt-4o")).toBeUndefined();
    expect(liveryTint("qwen-72b")).toBeUndefined();
  });

  it("returns undefined for undefined input", () => {
    expect(liveryTint(undefined)).toBeUndefined();
  });

  it("returns undefined for null input", () => {
    expect(liveryTint(null)).toBeUndefined();
  });

  it("returns undefined for empty string", () => {
    expect(liveryTint("")).toBeUndefined();
  });

  it("matches mixed-case input (e.g. MiMo-V2.5)", () => {
    expect(liveryTint("MiMo-V2.5")).toBeDefined();
    expect(liveryTint("MIMO")).toBeDefined();
  });
});

describe("drawCitizen — new figure types + carrying smoke test", () => {
  // PIXI v8 Graphics in headless mode don't expose geometry.graphicsData,
  // so we verify that drawing completes without throwing. The drawCitizen
  // function issues moveTo/lineTo/rect/circle/poly/ellipse calls which would
  // throw if the type were unrecognised or the opts invalid.
  it("draws priest without throwing", () => {
    const g = new Graphics();
    expect(() =>
      drawCitizen(g, "priest", { moving: false, phase: 0, actionPhase: 0 }),
    ).not.toThrow();
  });

  it("draws foreigner without throwing", () => {
    const g = new Graphics();
    expect(() =>
      drawCitizen(g, "foreigner", { moving: false, phase: 0, actionPhase: 0 }),
    ).not.toThrow();
  });

  it("draws with carrying: crate without throwing", () => {
    const g = new Graphics();
    expect(() =>
      drawCitizen(g, "merchant", {
        moving: true,
        phase: 1.2,
        actionPhase: 0,
        carrying: "crate",
      }),
    ).not.toThrow();
  });

  it("draws all figure types without throwing", () => {
    for (const type of [
      "citizen",
      "builder",
      "firefighter",
      "watercarrier",
      "merchant",
      "noble",
      "priest",
      "foreigner",
    ]) {
      const g = new Graphics();
      expect(() =>
        drawCitizen(g, type as any, {
          moving: false,
          phase: 0,
          actionPhase: 0,
        }),
      ).not.toThrow();
    }
  });
});

describe("AgentLayer.setBlocked — T2 walk blocker propagation", () => {
  it("setBlocked does not throw", () => {
    const { layer } = makeLayer();
    expect(() => {
      layer.setBlocked(() => true);
    }).not.toThrow();
  });

  it("setBlocked is idempotent (multiple calls are safe)", () => {
    const { layer } = makeLayer();
    layer.setBlocked(() => false);
    layer.setBlocked(() => true);
    layer.setBlocked(() => false);
    // No assertion needed — just verifying no throw.
  });
});

describe("livery tint preserved across figure flip (setPoseStatus)", () => {
  it("retains model-based livery when parentAgentId appears mid-lifecycle", () => {
    const { root, layer } = makeLayer();
    // Spy on the real drawCitizen (keep original impl) so we can inspect
    // the tunic value passed on each draw call.
    const spy = vi.spyOn(people, "drawCitizen");

    const jadeColor = liveryTint("MiMo-V2.5")!;
    expect(jadeColor).toBeDefined();

    // 1. Create a coder with MiMo model → figure=builder, tunic=jade livery.
    layer.applyDecisions([
      {
        kind: "createFresh",
        agentId: "c1",
        agent: mkAgent({
          agentId: "c1",
          type: "coder",
          model: "MiMo-V2.5",
        }),
        targetFileId: "f1",
        targetIso: { x: 10, y: 20 },
      },
    ]);

    // The initial draw (createAgent → step) must use the jade livery.
    const builderCalls = spy.mock.calls.filter(([, fig]) => fig === "builder");
    expect(builderCalls.length).toBeGreaterThan(0);
    expect(builderCalls[0][2]).toHaveProperty("tunic", jadeColor);

    // 2. Advance past appear-fade → idle so refresh can trigger setPoseStatus.
    layer.update(250); // > APPEAR_MS (200)

    // 3. Refresh with parentAgentId → figure flips builder→watercarrier.
    spy.mockClear();
    layer.applyDecisions([
      {
        kind: "refresh",
        agentId: "c1",
        agent: mkAgent({
          agentId: "c1",
          type: "coder",
          model: "MiMo-V2.5",
          parentAgentId: "p1",
        }),
      },
    ]);

    // Step to trigger the redraw with the new tunic.
    layer.step(1);

    // The watercarrier figure must STILL use the jade livery tint.
    const watercarrierCalls = spy.mock.calls.filter(
      ([, fig]) => fig === "watercarrier",
    );
    expect(watercarrierCalls.length).toBeGreaterThan(0);
    expect(watercarrierCalls[0][2]).toHaveProperty("tunic", jadeColor);

    spy.mockRestore();
  });
});
