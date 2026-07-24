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

  it("maps mini → watercarrier", () => {
    expect(figureForAgent(mkAgent({ type: "mini" }))).toBe("watercarrier");
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

  it("maps mini → watercarrier", () => {
    expect(figureForRole("mini")).toBe("watercarrier");
  });

  it("maps orchestrator → noble", () => {
    expect(figureForRole("orchestrator")).toBe("noble");
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

  it("figure names fall to the foreigner default", () => {
    // Role slugs map; kit figure names themselves are not roles.
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

describe("liveryTint — model family → tunic colour", () => {
  it("returns jade for Qwen and legacy MiMo alias (case-insensitive)", () => {
    const qwen = liveryTint("qwen3-coder");
    expect(qwen).toBeDefined();
    expect(typeof qwen).toBe("number");
    // MiMo is a legacy match alias for the same jade family.
    expect(liveryTint("MiMo-V2.5")).toBe(qwen);
    expect(liveryTint("MIMO")).toBe(qwen);
    expect(liveryTint("mimo")).toBe(qwen);
  });

  it("returns indigo for DeepSeek (same-family equality)", () => {
    const a = liveryTint("deepseek-chat");
    const b = liveryTint("deepseek-r1");
    expect(a).toBeDefined();
    expect(b).toBe(a);
  });

  it("returns terracotta for Claude family (same-family equality)", () => {
    const base = liveryTint("anthropic/claude-opus-4");
    expect(base).toBeDefined();
    expect(liveryTint("claude-sonnet-4")).toBe(base);
    expect(liveryTint("opus-4")).toBe(base);
    expect(liveryTint("fable-turbo")).toBe(base);
    expect(liveryTint("anthropic/claude-sonnet-4")).toBe(base);
  });

  it("matches bare Claude aliases via matchExact (sonnet/opus/haiku)", () => {
    const claude = liveryTint("claude-sonnet-4");
    expect(claude).toBeDefined();
    // Bare family tokens from some Agent.model paths.
    expect(liveryTint("sonnet")).toBe(claude);
    expect(liveryTint("opus")).toBe(claude);
    expect(liveryTint("haiku")).toBe(claude);
  });

  it("does not substring-match Claude on 'sonnet' inside another family id", () => {
    const claude = liveryTint("claude-sonnet-4");
    expect(claude).toBeDefined();
    // "sonnet" is matchExact only — a deepseek-prefixed id must resolve via
    // the DeepSeek substring, not Claude terracotta.
    const hybrid = liveryTint("deepseek-sonnet-hypothetical");
    expect(hybrid).toBe(liveryTint("deepseek-chat"));
    expect(hybrid).not.toBe(claude);
  });

  it("returns teal for OpenAI / gpt family (same-family equality)", () => {
    const openai = liveryTint("openai/gpt-4o-mini");
    expect(openai).toBeDefined();
    expect(liveryTint("gpt-4o")).toBe(openai);
    expect(liveryTint("gpt-3.5-turbo")).toBe(openai);
    expect(liveryTint("o1-preview")).toBe(openai);
    expect(liveryTint("o3-mini")).toBe(openai);
    expect(liveryTint("o4-mini")).toBe(openai);
    expect(liveryTint("gpt-5-preview")).toBe(openai);
  });

  it("returns slate for Grok / x-ai family (same-family equality)", () => {
    const grok = liveryTint("x-ai/grok-2");
    expect(grok).toBeDefined();
    expect(liveryTint("grok-beta")).toBe(grok);
  });

  it("cross-family tints are distinct", () => {
    const claude = liveryTint("claude-sonnet-4");
    const openai = liveryTint("openai/gpt-4o");
    const qwen = liveryTint("qwen3-coder");
    const deepseek = liveryTint("deepseek-chat");
    const grok = liveryTint("grok-2");
    expect(claude).toBeDefined();
    expect(openai).toBeDefined();
    expect(qwen).toBeDefined();
    expect(deepseek).toBeDefined();
    expect(grok).toBeDefined();
    expect(claude).not.toBe(openai);
    expect(claude).not.toBe(qwen);
    expect(openai).not.toBe(qwen);
    expect(claude).not.toBe(deepseek);
    expect(claude).not.toBe(grok);
    expect(openai).not.toBe(deepseek);
    expect(openai).not.toBe(grok);
    expect(qwen).not.toBe(deepseek);
    expect(qwen).not.toBe(grok);
    expect(deepseek).not.toBe(grok);
  });

  it("handles OpenRouter-prefixed ids via whole-string substring match", () => {
    expect(liveryTint("anthropic/claude-sonnet-4")).toBe(
      liveryTint("claude-sonnet-4"),
    );
    expect(liveryTint("openai/gpt-4o-mini")).toBe(liveryTint("gpt-4o"));
    expect(liveryTint("deepseek/deepseek-chat")).toBe(
      liveryTint("deepseek-chat"),
    );
    expect(liveryTint("qwen/qwen3-coder")).toBe(liveryTint("qwen3-coder"));
  });

  it("returns undefined for unknown models", () => {
    expect(liveryTint("llama-3.1-70b")).toBeUndefined();
    expect(liveryTint("mistral-large")).toBeUndefined();
    expect(liveryTint("some-local-model")).toBeUndefined();
  });

  it("does not false-match substring conflicts", () => {
    // "gpt" alone is not enough — EleutherAI-style / non-OpenAI architectures.
    expect(liveryTint("gpt-neox-20b")).toBeUndefined();
    // "mimo" substring without hyphen / exact token must not hit Qwen.
    expect(liveryTint("mimosa-7b")).toBeUndefined();
    // "grok" without hyphen / path boundary must not hit Grok.
    expect(liveryTint("grokfast")).toBeUndefined();
    // OpenRouter auto router is not a model family.
    expect(liveryTint("openrouter/auto")).toBeUndefined();
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

describe("livery tint re-derived on model-only change (setPoseStatus)", () => {
  it("re-derives tunic when model changes but figure stays the same", () => {
    const { layer } = makeLayer();
    const spy = vi.spyOn(people, "drawCitizen");

    const jadeColor = liveryTint("MiMo-V2.5")!;
    expect(jadeColor).toBeDefined();

    // 1. Create a coder with NO model → figure=builder, tunic=figure default.
    layer.applyDecisions([
      {
        kind: "createFresh",
        agentId: "c1",
        agent: mkAgent({ agentId: "c1", type: "coder" }),
        targetFileId: "f1",
        targetIso: { x: 10, y: 20 },
      },
    ]);

    // Initial draw uses default tunic (no model → liveryTint returns undefined).
    const builderCalls = spy.mock.calls.filter(([, fig]) => fig === "builder");
    expect(builderCalls.length).toBeGreaterThan(0);
    const defaultTunic = builderCalls[0][2].tunic;
    expect(defaultTunic).not.toBe(jadeColor);

    // 2. Advance past appear-fade → idle so refresh can trigger setPoseStatus.
    layer.update(250); // > APPEAR_MS (200)

    // 3. Refresh with SAME type but model="MiMo-V2.5" → tunic should flip to jade.
    spy.mockClear();
    layer.applyDecisions([
      {
        kind: "refresh",
        agentId: "c1",
        agent: mkAgent({
          agentId: "c1",
          type: "coder",
          model: "MiMo-V2.5",
        }),
      },
    ]);

    // Step to trigger the redraw with the new tunic.
    layer.step(1);

    // The builder figure must NOW use the jade livery tint.
    const refreshedCalls = spy.mock.calls.filter(([, fig]) => fig === "builder");
    expect(refreshedCalls.length).toBeGreaterThan(0);
    expect(refreshedCalls[0][2]).toHaveProperty("tunic", jadeColor);

    spy.mockRestore();
  });
});

describe("livery tint preserved across figure flip (setPoseStatus)", () => {
  it("retains model-based livery when parentAgentId appears mid-lifecycle", () => {
    const { layer } = makeLayer();
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
