import { describe, it, expect } from "vitest";
import { Disaster } from "./growthEffects";
import { worstSinSeverity, buildingChanged } from "./diffCity";
import type { Building, SinSeverity, UrbanSin } from "../../types/city";

// On-map DISASTER overlay tests. The overlay is DATA-DRIVEN: it exists iff a
// building has urban sins, is keyed on the WORST severity, and AUTO-CLEARS via
// the diff rebuild when the sins clear. These exercise:
//   - the worst-severity helper (the single source of truth for the overlay kind)
//   - the Disaster overlay composition by severity (headless PIXI — no WebGL)
//   - the auto-clear seam: a sins-clear is a rebuild, and a cleared building maps
//     to NO overlay (worstSinSeverity → null).
//
// PIXI v8 Container/Graphics are plain scene-graph objects (no GL to construct +
// record geometry), so Disaster (which composes kit Flame/Smoke) drives headless.

function sin(severity: SinSeverity, sinId = "s1"): UrbanSin {
  return { sinId, severity, description: "", autoDetectable: true };
}

function mkBuilding(overrides: Partial<Building> = {}): Building {
  return {
    fileId: "fid-1",
    filePath: "src/a.ts",
    districtId: "core",
    purpose: "house",
    purposeSource: "default",
    linesOfCode: 10,
    visualTier: "kalybe",
    coords: { x: 5, y: 5 },
    status: "normal",
    label: "a.ts",
    description: "",
    lastModified: "",
    sins: [],
    notes: [],
    ...overrides,
  };
}

describe("worstSinSeverity — overlay kind (single source of truth)", () => {
  it("returns null for a building with no sins (no overlay)", () => {
    expect(worstSinSeverity(mkBuilding())).toBeNull();
  });

  it("collapses to the WORST severity (none < smoke < fire < inferno)", () => {
    expect(worstSinSeverity(mkBuilding({ sins: [sin("smoke")] }))).toBe("smoke");
    expect(worstSinSeverity(mkBuilding({ sins: [sin("fire")] }))).toBe("fire");
    expect(
      worstSinSeverity(mkBuilding({ sins: [sin("inferno")] })),
    ).toBe("inferno");
    // mixed → the worst wins regardless of order
    expect(
      worstSinSeverity(
        mkBuilding({
          sins: [sin("smoke", "a"), sin("inferno", "b"), sin("fire", "c")],
        }),
      ),
    ).toBe("inferno");
    expect(
      worstSinSeverity(
        mkBuilding({ sins: [sin("inferno", "a"), sin("smoke", "b")] }),
      ),
    ).toBe("inferno");
  });

  it("normalizes an unknown/Oracle severity to the smoke floor (rank 1)", () => {
    const weird = { ...sin("smoke"), severity: "rumor" as SinSeverity };
    expect(worstSinSeverity(mkBuilding({ sins: [weird] }))).toBe("smoke");
  });
});

describe("Disaster overlay — kit Flame/Smoke composition by severity", () => {
  it("smoke → smoke wisps only (no flame), builds + animates without throwing", () => {
    const d = new Disaster("smoke", 40, 120);
    // Composed kit parts are children of the overlay node.
    expect(d.node.children.length).toBeGreaterThan(0);
    // Visible by default so the step driver runs (the renderer LOD-gates it).
    d.node.visible = true;
    for (let i = 0; i < 30; i++) d.update(i / 30, 1 / 30);
    d.node.destroy({ children: true });
  });

  it("fire → flame + smoke, inferno → bigger flame + more smoke (tinted)", () => {
    const fire = new Disaster("fire", 40, 120);
    const inferno = new Disaster("inferno", 40, 120);
    // Inferno is the most engulfed → strictly more composed parts than fire.
    expect(inferno.node.children.length).toBeGreaterThan(
      fire.node.children.length,
    );
    fire.node.visible = true;
    inferno.node.visible = true;
    for (let i = 0; i < 20; i++) {
      fire.update(i / 30, 1 / 30);
      inferno.update(i / 30, 1 / 30);
    }
    fire.node.destroy({ children: true });
    inferno.node.destroy({ children: true });
  });

  it("LOD gate: a hidden overlay skips its per-step redraw (early return)", () => {
    const d = new Disaster("inferno", 40, 120);
    d.node.visible = false; // simulate a far-zoom LOD hide
    // Drive it — must not throw and must not redraw (we just assert it's a no-op
    // path; the visible=false guard returns before touching the kit parts).
    for (let i = 0; i < 10; i++) d.update(i / 30, 1 / 30);
    expect(d.node.visible).toBe(false);
    d.node.destroy({ children: true });
  });

  it("clamps a degenerate footprint so geometry stays sane", () => {
    const d = new Disaster("fire", 0, 0);
    expect(d.node.children.length).toBeGreaterThan(0);
    d.node.visible = true;
    d.update(0, 0.05);
    d.node.destroy({ children: true });
  });
});

describe("Disaster auto-clear — driven by the diff rebuild", () => {
  it("appearing sins (none → fire) trigger a node rebuild", () => {
    expect(
      buildingChanged(mkBuilding(), mkBuilding({ sins: [sin("fire")] })),
    ).toBe(true);
  });

  it("CLEARING sins (fire → none) triggers a rebuild → overlay removed", () => {
    // The auto-clear seam: when a file is FIXED its worst-severity rank drops to
    // 0, so buildingChanged is true → the node is rebuilt; the rebuilt node maps
    // to NO overlay (worstSinSeverity → null). No separate clearing mechanism.
    const burning = mkBuilding({ sins: [sin("fire")] });
    const fixed = mkBuilding({ sins: [] });
    expect(buildingChanged(burning, fixed)).toBe(true);
    expect(worstSinSeverity(burning)).toBe("fire");
    expect(worstSinSeverity(fixed)).toBeNull();
  });

  it("worsening severity (smoke → inferno) rebuilds to a bigger overlay", () => {
    const a = mkBuilding({ sins: [sin("smoke")] });
    const b = mkBuilding({ sins: [sin("inferno")] });
    expect(buildingChanged(a, b)).toBe(true);
    expect(worstSinSeverity(a)).toBe("smoke");
    expect(worstSinSeverity(b)).toBe("inferno");
  });

  it("a cosmetic reorder (worst unchanged) does NOT rebuild the overlay", () => {
    const a = mkBuilding({ sins: [sin("fire", "x"), sin("smoke", "y")] });
    const b = mkBuilding({ sins: [sin("smoke", "y"), sin("fire", "x")] });
    expect(buildingChanged(a, b)).toBe(false);
    expect(worstSinSeverity(a)).toBe("fire");
    expect(worstSinSeverity(b)).toBe("fire");
  });
});
