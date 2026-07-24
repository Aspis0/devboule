// Pins the shared contact-shadow policy so the historical 0.13-vs-0.32
// drift (buildShadow hard-coded 0.13 while ALPHA.shadow claimed 0.32)
// cannot return silently.

import { describe, it, expect } from "vitest";
import { CONTACT_SHADOW } from "./contactShadow";
import { ALPHA } from "./palette";

describe("CONTACT_SHADOW — single shadow model", () => {
  it("alpha is in the UH-tree-matched band (~0.28–0.35)", () => {
    expect(CONTACT_SHADOW.alpha).toBeGreaterThanOrEqual(0.28);
    expect(CONTACT_SHADOW.alpha).toBeLessThanOrEqual(0.35);
  });

  it("offset is a soft centred pool (no hard cast direction)", () => {
    // Tree art in prop-0 is a soft contact pool under the canopy, not a
    // hard SE/NW drop — policy must stay near zero.
    expect(Math.abs(CONTACT_SHADOW.offsetX)).toBeLessThanOrEqual(1);
    expect(Math.abs(CONTACT_SHADOW.offsetY)).toBeLessThanOrEqual(1);
  });

  it("ALPHA.shadow aliases CONTACT_SHADOW.alpha (no dual sources)", () => {
    expect(ALPHA.shadow).toBe(CONTACT_SHADOW.alpha);
  });

  it("is stronger than the old floating 0.13 building ellipse", () => {
    // Regression: buildings/index.ts once hard-coded alpha 0.13 while the
    // palette claimed 0.32 — buildings floated next to planted trees.
    expect(CONTACT_SHADOW.alpha).toBeGreaterThan(0.13);
  });
});
