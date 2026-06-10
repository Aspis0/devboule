// PURE snapshot + behavior tests for the versioned prompt contract.

import { describe, it, expect } from "vitest";
import {
  DESIGN_PROMPT_VERSION,
  DESIGN_SYSTEM_PROMPT_V1,
  buildGeneratePrompt,
  buildEditPrompt,
} from "./prompt";

describe("DESIGN_SYSTEM_PROMPT_V1 contract", () => {
  it("is a versioned, stable constant", () => {
    expect(DESIGN_PROMPT_VERSION).toBe(1);
    expect(DESIGN_SYSTEM_PROMPT_V1).toMatchSnapshot();
  });

  it("forbids scripts, on* handlers, JSX, and positioning", () => {
    const p = DESIGN_SYSTEM_PROMPT_V1.toLowerCase();
    expect(p).toContain("<script>");
    expect(p).toContain("on* event handler");
    expect(p).toContain("jsx");
    expect(p).toContain("position");
    expect(p).toContain("z-index");
    expect(p).toContain("data-node-id");
  });
});

describe("buildGeneratePrompt", () => {
  it("snapshots a basic generation prompt", () => {
    expect(buildGeneratePrompt("a pricing section")).toMatchSnapshot();
  });

  it("includes the user instruction (trimmed)", () => {
    const out = buildGeneratePrompt("  a hero banner  ");
    expect(out).toContain("a hero banner");
    expect(out).not.toContain("  a hero banner  ");
  });

  it("inserts a context block only when provided", () => {
    const without = buildGeneratePrompt("x");
    expect(without).not.toContain("CONTEXT");
    const withCtx = buildGeneratePrompt("x", { context: "brand color #c2410c" });
    expect(withCtx).toContain("CONTEXT");
    expect(withCtx).toContain("brand color #c2410c");
  });

  it("ignores a blank context", () => {
    expect(buildGeneratePrompt("x", { context: "   " })).not.toContain("CONTEXT");
  });
});

describe("buildEditPrompt", () => {
  it("snapshots a single-node edit prompt", () => {
    expect(
      buildEditPrompt(
        '<button data-node-id="cta">Go</button>',
        "make it the brand accent",
      ),
    ).toMatchSnapshot();
  });

  it("embeds the current markup and the instruction", () => {
    const out = buildEditPrompt(
      '<button data-node-id="cta">Go</button>',
      "bigger",
    );
    expect(out).toContain('data-node-id="cta"');
    expect(out).toContain("bigger");
    expect(out).toContain("PRESERVED");
  });
});
