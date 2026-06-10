import { describe, it, expect } from "vitest";
import { computeDragCommit, otherRects } from "./canvas/dragCommit";
import type { DesignManifest, DesignNodePlacement } from "../../types/design";

function placement(over: Partial<DesignNodePlacement> = {}): DesignNodePlacement {
  return { x: 0, y: 0, z: 1, w: 200, h: "auto", kind: "html", ...over };
}
function manifest(nodes: Record<string, DesignNodePlacement>): DesignManifest {
  return { schemaVersion: 1, nodes };
}

describe("computeDragCommit — move", () => {
  it("applies the delta with grid snapping", () => {
    const m = manifest({ hero: placement({ x: 0, y: 0 }) });
    const next = computeDragCommit({
      manifest: m,
      id: "hero",
      mode: "move",
      dx: 13,
      dy: 5,
      grid: 8,
      others: [],
    });
    expect(next.nodes.hero.x).toBe(16); // 13 -> snap 16
    expect(next.nodes.hero.y).toBe(8); // 5 -> snap 8
  });

  it("nudges by smart-guide alignment to another node's left edge", () => {
    const m = manifest({
      hero: placement({ x: 100, y: 0, w: 200 }),
      cta: placement({ x: 0, y: 0, w: 100 }),
    });
    // Move cta so its snapped-x is 98; hero left=100 -> guide snaps +2.
    const next = computeDragCommit({
      manifest: m,
      id: "cta",
      mode: "move",
      dx: 98, // 0 + 98, grid disabled -> 98
      dy: 0,
      grid: 0,
      others: otherRects(m, "cta"),
    });
    expect(next.nodes.cta.x).toBe(100); // 98 + guide(+2)
  });

  it("returns the same manifest for a missing id", () => {
    const m = manifest({ hero: placement() });
    const next = computeDragCommit({
      manifest: m,
      id: "ghost",
      mode: "move",
      dx: 10,
      dy: 10,
      grid: 8,
      others: [],
    });
    expect(next).toBe(m);
  });
});

describe("computeDragCommit — resize", () => {
  it("sets a snapped width and keeps auto height for horizontal-only resize", () => {
    const m = manifest({ hero: placement({ w: 200, h: "auto" }) });
    const next = computeDragCommit({
      manifest: m,
      id: "hero",
      mode: "resize",
      dx: 53,
      dy: 0,
      grid: 8,
      others: [],
    });
    expect(next.nodes.hero.w).toBe(256); // 253 -> snap 256
    expect(next.nodes.hero.h).toBe("auto");
  });

  it("pins a numeric height when dragged vertically", () => {
    const m = manifest({ hero: placement({ w: 200, h: 100 }) });
    const next = computeDragCommit({
      manifest: m,
      id: "hero",
      mode: "resize",
      dx: 0,
      dy: 56,
      grid: 8,
      others: [],
    });
    expect(next.nodes.hero.h).toBe(160); // 156 -> snap 160
  });

  it("never shrinks width below 1px", () => {
    const m = manifest({ hero: placement({ w: 10 }) });
    const next = computeDragCommit({
      manifest: m,
      id: "hero",
      mode: "resize",
      dx: -9999,
      dy: 0,
      grid: 0,
      others: [],
    });
    expect(next.nodes.hero.w).toBeGreaterThanOrEqual(1);
  });
});

describe("otherRects", () => {
  it("excludes the given id and resolves auto height to 0", () => {
    const m = manifest({
      a: placement({ x: 1, h: "auto" }),
      b: placement({ x: 2, h: 50 }),
    });
    const rects = otherRects(m, "a");
    expect(rects).toHaveLength(1);
    expect(rects[0].id).toBe("b");
    expect(rects[0].h).toBe(50);
  });

  it("resolves a moving node's auto height to 0 in its own rect", () => {
    const m = manifest({ a: placement({ h: "auto" }), b: placement() });
    const rects = otherRects(m, "b");
    expect(rects[0].h).toBe(0);
  });
});
