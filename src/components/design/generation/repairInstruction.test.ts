import { describe, it, expect } from "vitest";
import { buildRepairInstruction } from "./repairInstruction";
import type { ViolationCode } from "./contractValidator";

function v(...cs: ViolationCode[]) {
  return cs.map((code) => ({ code }));
}

describe("buildRepairInstruction", () => {
  it("returns '' for no violations", () => {
    expect(buildRepairInstruction([])).toBe("");
  });

  it("mentions the foster-parent tags for FOSTER_PARENTED_ROOT", () => {
    const out = buildRepairInstruction(v("FOSTER_PARENTED_ROOT"));
    expect(out).toContain("<tr>");
    expect(out).toContain("top-level element");
  });

  it("instructs exactly one top-level element for MULTIPLE_TOP_LEVEL", () => {
    const out = buildRepairInstruction(v("MULTIPLE_TOP_LEVEL"));
    expect(out).toContain("EXACTLY ONE top-level element");
  });

  it("lists the host-owned props for POSITIONAL_CSS_ON_ROOT", () => {
    const out = buildRepairInstruction(v("POSITIONAL_CSS_ON_ROOT"));
    expect(out).toContain("position");
    expect(out).toContain("host owns placement");
  });

  it("de-duplicates repeated codes", () => {
    const out = buildRepairInstruction(
      v("MULTIPLE_TOP_LEVEL", "MULTIPLE_TOP_LEVEL", "MULTIPLE_TOP_LEVEL"),
    );
    // The bullet appears exactly once.
    const matches = out.match(/EXACTLY ONE top-level element/g) ?? [];
    expect(matches).toHaveLength(1);
  });

  it("is order-independent (same code-set -> identical output)", () => {
    const a = buildRepairInstruction(
      v("POSITIONAL_CSS_ON_ROOT", "FOSTER_PARENTED_ROOT"),
    );
    const b = buildRepairInstruction(
      v("FOSTER_PARENTED_ROOT", "POSITIONAL_CSS_ON_ROOT"),
    );
    expect(a).toBe(b);
  });

  it("emits one bullet per distinct code", () => {
    const out = buildRepairInstruction(
      v("FOSTER_PARENTED_ROOT", "POSITIONAL_CSS_ON_ROOT", "EMPTY"),
    );
    const bullets = out.split("\n").filter((l) => l.startsWith("- "));
    expect(bullets).toHaveLength(3);
  });
});
