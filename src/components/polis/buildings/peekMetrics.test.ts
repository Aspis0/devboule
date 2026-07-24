// Parity: peekBuildingParts / metricsFromBounds must return IDENTICAL hw/depth/foot
// to the full buildBuildingParts slow path for a sample of purposes/levels/salts.
//
// The in-place atlas-HIT fast path (updateBuildingNodeInPlace) derives metrics via
// metricsFromBounds(variant.frame) + variant.foot and must not drift from the
// values a full kit rebuild would produce.

import { describe, it, expect, afterEach } from "vitest";
import type { Building } from "../../../types/city";
import { getProfile, tierScale } from "../palette";
import {
  buildBuildingParts,
  metricsFromBounds,
  peekBuildingParts,
} from "./index";

function mkBuilding(
  purpose: string,
  visualTier: string,
  over: Partial<Building> = {},
): Building {
  return {
    fileId: over.fileId ?? `f-${purpose}-${visualTier}`,
    filePath: over.filePath ?? `src/${purpose}.ts`,
    districtId: "d1",
    purpose,
    purposeSource: "grounded",
    linesOfCode: 100,
    visualTier,
    coords: { x: 0, y: 0 },
    status: "idle",
    label: `${purpose}.ts`,
    description: "",
    lastModified: "2026-01-01",
    agentPresent: undefined,
    suspectOfCardId: undefined,
    sins: [],
    provider: over.provider,
    ...over,
  } as Building;
}

/** Dispose every live object a BuiltParts / LiveParts may still hold. */
function disposeSlow(built: ReturnType<typeof buildBuildingParts>): void {
  if (!built.staticBody.destroyed) built.staticBody.destroy({ children: true });
  if (!built.shadow.destroyed) built.shadow.destroy();
  if (built.pennant && !built.pennant.destroyed) built.pennant.destroy();
  for (const a of built.anims) {
    a.node.removeFromParent();
    if (!a.node.destroyed) a.node.destroy({ children: true });
  }
}

function disposeLive(live: ReturnType<typeof peekBuildingParts>): void {
  if (live.pennant && !live.pennant.destroyed) live.pennant.destroy();
  for (const a of live.anims) {
    a.node.removeFromParent();
    if (!a.node.destroyed) a.node.destroy({ children: true });
  }
}

const SAMPLES: Array<{ purpose: string; tier: string; salt: number }> = [
  { purpose: "house", tier: "kalybe", salt: 0 },
  { purpose: "house", tier: "synoikia", salt: 1 },
  { purpose: "house", tier: "megaron", salt: 2 },
  { purpose: "house", tier: "mnemeion", salt: 3 },
  { purpose: "workshop", tier: "oikia", salt: 0 },
  { purpose: "workshop", tier: "synoikia", salt: 2 },
  { purpose: "warehouse", tier: "synoikia", salt: 1 },
  { purpose: "market", tier: "megaron", salt: 0 },
  { purpose: "temple", tier: "mnemeion", salt: 0 },
  { purpose: "lighthouse", tier: "megaron", salt: 0 },
  { purpose: "library", tier: "synoikia", salt: 1 },
  // Avoid "unknown" — its builder attaches a PIXI Text label that needs `document`
  // (canvas text metrics) which the node vitest env does not provide.
  { purpose: "baths", tier: "oikia", salt: 1 },
  { purpose: "tower", tier: "megaron", salt: 0 },
];

describe("metricsFromBounds — same formulas as buildBuildingParts", () => {
  afterEach(() => {
    // nothing global; per-test dispose handles PIXI trees
  });

  for (const s of SAMPLES) {
    it(`frame metrics match slow path for ${s.purpose}/${s.tier}/s${s.salt}`, () => {
      const b = mkBuilding(s.purpose, s.tier);
      const profile = getProfile(b.purpose);
      const scale = tierScale(b.visualTier);
      const slow = buildBuildingParts(b, profile, scale, s.salt);

      // Atlas stores body local bounds as `frame` at bake time (anims already
      // detached in buildBuildingParts — same state the atlas captures).
      const bounds = slow.staticBody.getLocalBounds();
      const frame = {
        x: bounds.x,
        y: bounds.y,
        width: bounds.width,
        height: bounds.height,
      };
      const fast = metricsFromBounds(frame);

      expect(fast.hw).toBe(slow.hw);
      expect(fast.depth).toBe(slow.depth);

      disposeSlow(slow);
    });
  }
});

describe("peekBuildingParts — metrics parity with buildBuildingParts", () => {
  for (const s of SAMPLES) {
    it(`peek hw/depth/foot == slow for ${s.purpose}/${s.tier}/s${s.salt}`, () => {
      const b = mkBuilding(s.purpose, s.tier, { provider: "cloudflare" });
      const profile = getProfile(b.purpose);
      const scale = tierScale(b.visualTier);

      const slow = buildBuildingParts(b, profile, scale, s.salt);
      const peek = peekBuildingParts(b, profile, scale, s.salt);

      expect(peek.hw).toBe(slow.hw);
      expect(peek.depth).toBe(slow.depth);
      expect(peek.foot).toEqual(slow.foot);
      // Both paths produce a pennant for a known provider.
      expect(peek.pennant).not.toBeNull();
      expect(slow.pennant).not.toBeNull();
      // Peek must not retain a static body (destroyed inside peek).
      // (We only have live parts — no staticBody field by construction.)

      disposeSlow(slow);
      disposeLive(peek);
    });
  }

  it("peek does not build a retained shadow and destroys the static body", () => {
    const b = mkBuilding("house", "synoikia");
    const profile = getProfile(b.purpose);
    const scale = tierScale(b.visualTier);
    const peek = peekBuildingParts(b, profile, scale, 0);
    // LiveParts has no staticBody/shadow fields — shape guard.
    expect("staticBody" in peek).toBe(false);
    expect("shadow" in peek).toBe(false);
    expect(peek.foot[0]).toBeGreaterThan(0);
    expect(peek.hw).toBeGreaterThan(0);
    expect(peek.depth).toBeGreaterThan(0);
    disposeLive(peek);
  });
});
