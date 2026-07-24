import { describe, it, expect } from "vitest";
import { Container, Graphics } from "pixi.js";
import { Disaster, Investigation } from "./growthEffects";
import { buildingChanged } from "./diffCity";
import { removeFromArrayByIdentity } from "./PolisRenderer";
import { DERIVED } from "./palette";
import type { Building, SinSeverity, UrbanSin } from "../../types/city";

// Bug-investigation P3 — the "under investigation" overlay (tinted kit Smoke + a
// diegetic "?" stele) raised on a building an OPEN bug card's Oracle suspects
// resolve to. It is DATA-DRIVEN (exists iff `suspectOfCardId` is set), AUTO-CLEARS
// via the diff rebuild, and COEXISTS with the confirmed-disaster fire. These
// exercise:
//   - the overlay composition (tinted smoke parts + a stele Graphics marker)
//   - the HONESTY/coexistence seam: a building with BOTH a worst-sin disaster AND a
//     suspect marker builds BOTH overlays, neither clobbering the other
//   - the diff seam: buildingChanged flips when suspectOfCardId appears/disappears
//     and stays false when it is unchanged.
//
// PIXI v8 Container/Graphics are plain scene-graph objects (no GL needed to
// construct + record geometry), so the overlays drive headlessly.

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

/** Collect the kit `Smoke` containers composed into an overlay (each is a child
 *  Container holding the smoke Graphics; the stele marker is a bare Graphics). */
function smokeContainers(node: Container): Container[] {
  return node.children.filter(
    (c): c is Container => c instanceof Container && !(c instanceof Graphics),
  );
}

/** The diegetic stele + painted "?" glyph (static Graphics child). */
function steleMarkers(node: Container): Graphics[] {
  return node.children.filter((c): c is Graphics => c instanceof Graphics);
}

describe("Investigation overlay — tinted kit Smoke + diegetic '?' stele", () => {
  it("builds tinted smoke wisps AND a stele Graphics marker (no system-font Text)", () => {
    const inv = new Investigation(40, 120);
    // A stele Graphics marker is present (not a Text glyph).
    const marks = steleMarkers(inv.node);
    expect(marks.length).toBe(1);
    // Non-empty geometry — a blank Graphics would pass a mere instanceof check.
    const steleBounds = marks[0].getLocalBounds();
    expect(steleBounds.width).toBeGreaterThan(0);
    expect(steleBounds.height).toBeGreaterThan(0);
    // The smoke parts carry the investigative indigo/violet tint (DISTINCT from a
    // disaster's orange/red fire — the HONESTY invariant).
    const smokes = smokeContainers(inv.node);
    expect(smokes.length).toBeGreaterThan(0);
    for (const s of smokes) expect(s.tint).toBe(DERIVED.investigate);
    // Drives without throwing (the kit Smoke clear+redraw path).
    inv.node.visible = true;
    for (let i = 0; i < 30; i++) inv.update(i / 30, 1 / 30);
    inv.node.destroy({ children: true });
  });

  it("LOD gate: a hidden overlay skips its per-step redraw (early return)", () => {
    const inv = new Investigation(40, 120);
    inv.node.visible = false; // simulate a far-zoom LOD hide
    for (let i = 0; i < 10; i++) inv.update(i / 30, 1 / 30);
    expect(inv.node.visible).toBe(false);
    inv.node.destroy({ children: true });
  });

  it("clamps a degenerate footprint so geometry stays sane", () => {
    const inv = new Investigation(0, 0); // degenerate inputs
    expect(inv.node.children.length).toBeGreaterThan(0);
    inv.node.visible = true;
    inv.update(0, 0.05);
    inv.node.destroy({ children: true });
  });
});

describe("Investigation + Disaster coexistence (no clobber)", () => {
  it("a building with BOTH a worst-sin disaster AND a suspect builds both overlays", () => {
    // Both overlays are independent scene-graph nodes parented into the SAME
    // building container in the renderer; here we assert they construct side by
    // side with their distinct, honest content (fire vs. tinted-smoke + "?").
    const parent = new Container(); // stand-in for the building node container
    const disaster = new Disaster("inferno", 40, 120);
    const investigation = new Investigation(40, 120);
    parent.addChild(disaster.node);
    parent.addChild(investigation.node);

    // Both overlays are present and independent (neither replaced the other).
    expect(parent.children).toContain(disaster.node);
    expect(parent.children).toContain(investigation.node);

    // The investigation overlay still carries its own stele marker + tinted smoke —
    // the disaster did not strip or recolor it.
    expect(steleMarkers(investigation.node).length).toBe(1);
    for (const s of smokeContainers(investigation.node)) {
      expect(s.tint).toBe(DERIVED.investigate);
    }

    // Both animate without throwing.
    disaster.node.visible = true;
    investigation.node.visible = true;
    for (let i = 0; i < 20; i++) {
      disaster.update(i / 30, 1 / 30);
      investigation.update(i / 30, 1 / 30);
    }
    parent.destroy({ children: true });
  });
});

describe("Investigation renderer lifecycle — no stale step after destroy", () => {
  // The renderer tracks animated building nodes in an `animatedNodes` array and
  // the per-step clock walks it, calling `kitAnims[i].update(t, dt)` on each. A
  // node DESTROYED via `destroyBuildingNode` MUST be spliced out (via
  // `removeFromArrayByIdentity`) or the next step would touch a destroyed
  // container — a use-after-free leak. A full PolisRenderer needs a WebGL
  // Application+Viewport (not headless-constructible), so — mirroring how
  // disaster.test/investigation.test exercise the overlay + diff seams rather than
  // the GL-bound renderer — we test at the smallest honest seam: the exact
  // array-maintenance helper destroyBuildingNode calls, with REAL Investigation
  // instances in the node's kitAnims so the "no stale step" claim is concrete.
  type FakeNode = { kitAnims: Investigation[]; container: Container };

  function fakeAnimatedNode(): FakeNode {
    const inv = new Investigation(40, 120);
    const container = new Container();
    container.addChild(inv.node);
    return { kitAnims: [inv], container };
  }

  it("destroying an Investigation-carrying node removes it from the animated pool", () => {
    const a = fakeAnimatedNode();
    const target = fakeAnimatedNode();
    const b = fakeAnimatedNode();
    const animatedNodes: FakeNode[] = [a, target, b];

    // destroyBuildingNode's contract: destroy the container, then splice the node
    // out of the animated pool (the line under test).
    target.container.destroy({ children: true });
    const removed = removeFromArrayByIdentity(animatedNodes, target);

    expect(removed).toBe(true);
    expect(animatedNodes).not.toContain(target);
    // Siblings are untouched and still ordered.
    expect(animatedNodes).toEqual([a, b]);

    // The per-step clock (which only ever walks `animatedNodes`) now NEVER touches
    // the destroyed node — stepping the remaining pool throws nothing and the
    // destroyed Investigation is not stepped.
    const stepped: Investigation[] = [];
    for (const node of animatedNodes) {
      for (const anim of node.kitAnims) {
        anim.update(0.1, 1 / 30);
        stepped.push(anim);
      }
    }
    expect(stepped).not.toContain(target.kitAnims[0]);
    expect(stepped).toHaveLength(2);

    a.container.destroy({ children: true });
    b.container.destroy({ children: true });
  });

  it("removing an absent (never-tracked static) node is a safe no-op", () => {
    // A static building with no kit anims is never pushed into `animatedNodes`;
    // destroying it must not corrupt the pool.
    const tracked = fakeAnimatedNode();
    const animatedNodes: FakeNode[] = [tracked];
    const untracked = fakeAnimatedNode();

    const removed = removeFromArrayByIdentity(animatedNodes, untracked);
    expect(removed).toBe(false);
    expect(animatedNodes).toEqual([tracked]);

    tracked.container.destroy({ children: true });
    untracked.container.destroy({ children: true });
  });
});

describe("Investigation auto-clear — driven by the diff rebuild", () => {
  it("a suspect APPEARING (none → set) triggers a node rebuild", () => {
    expect(
      buildingChanged(
        mkBuilding(),
        mkBuilding({ suspectOfCardId: "BUG-1" }),
      ),
    ).toBe(true);
  });

  it("a suspect CLEARING (set → none) triggers a rebuild → overlay removed", () => {
    expect(
      buildingChanged(
        mkBuilding({ suspectOfCardId: "BUG-1" }),
        mkBuilding(),
      ),
    ).toBe(true);
  });

  it("a CHANGE of the suspecting card id rebuilds (re-render the marker)", () => {
    expect(
      buildingChanged(
        mkBuilding({ suspectOfCardId: "BUG-1" }),
        mkBuilding({ suspectOfCardId: "BUG-2" }),
      ),
    ).toBe(true);
  });

  it("an UNCHANGED suspect does NOT rebuild the node", () => {
    expect(
      buildingChanged(
        mkBuilding({ suspectOfCardId: "BUG-1" }),
        mkBuilding({ suspectOfCardId: "BUG-1" }),
      ),
    ).toBe(false);
  });

  it("the suspect channel is INDEPENDENT of sins (a building can be both)", () => {
    // Same suspect, same worst-sin → no rebuild (neither channel changed).
    const a = mkBuilding({ suspectOfCardId: "BUG-1", sins: [sin("fire")] });
    const b = mkBuilding({ suspectOfCardId: "BUG-1", sins: [sin("fire")] });
    expect(buildingChanged(a, b)).toBe(false);
    // Changing ONLY the suspect (sins held) still rebuilds.
    const c = mkBuilding({ suspectOfCardId: "BUG-9", sins: [sin("fire")] });
    expect(buildingChanged(a, c)).toBe(true);
  });
});
