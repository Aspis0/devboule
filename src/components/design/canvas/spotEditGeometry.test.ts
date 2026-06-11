// Exhaustive unit tests for the PURE Spot Edit geometry. No DOM needed.

import { describe, it, expect } from "vitest";
import type { NodeRect, Point } from "../../../types/design";
import {
  MIN_REGION_VERTICES,
  bboxOf,
  insertMidpoint,
  pointInPolygon,
  polygonIntersectsRect,
  rectToPts,
  removeVertex,
  screenToWorld,
  worldToScreen,
} from "./spotEditGeometry";

const rect = (x: number, y: number, w: number, h: number): NodeRect => ({
  id: "n",
  x,
  y,
  w,
  h,
  z: 1,
});

describe("rectToPts", () => {
  it("returns 4 clockwise corners from top-left", () => {
    expect(rectToPts(10, 20, 30, 40)).toEqual([
      { x: 10, y: 20 },
      { x: 40, y: 20 },
      { x: 40, y: 60 },
      { x: 10, y: 60 },
    ]);
  });

  it("handles a zero-size rect (a point)", () => {
    expect(rectToPts(5, 5, 0, 0)).toEqual([
      { x: 5, y: 5 },
      { x: 5, y: 5 },
      { x: 5, y: 5 },
      { x: 5, y: 5 },
    ]);
  });
});

describe("bboxOf", () => {
  it("computes the axis-aligned bounds of a point set", () => {
    const pts: Point[] = [
      { x: 10, y: 50 },
      { x: 40, y: 5 },
      { x: 0, y: 30 },
    ];
    expect(bboxOf(pts)).toEqual({ x: 0, y: 5, w: 40, h: 45 });
  });

  it("returns a zero box at origin for an empty set", () => {
    expect(bboxOf([])).toEqual({ x: 0, y: 0, w: 0, h: 0 });
  });
});

describe("insertMidpoint", () => {
  it("splices the edge midpoint AFTER index i", () => {
    const pts = rectToPts(0, 0, 100, 100); // 4 corners
    const next = insertMidpoint(pts, 0); // midpoint of edge 0->1 = (50,0)
    expect(next).toHaveLength(5);
    expect(next[1]).toEqual({ x: 50, y: 0 });
    // original points are preserved around it
    expect(next[0]).toEqual(pts[0]);
    expect(next[2]).toEqual(pts[1]);
  });

  it("wraps the last edge back to vertex 0", () => {
    const pts = rectToPts(0, 0, 10, 10);
    const next = insertMidpoint(pts, 3); // edge 3->0 midpoint = (0,5)
    expect(next).toHaveLength(5);
    expect(next[4]).toEqual({ x: 0, y: 5 });
  });

  it("returns a copy unchanged for an out-of-range index", () => {
    const pts = rectToPts(0, 0, 10, 10);
    expect(insertMidpoint(pts, 9)).toEqual(pts);
    expect(insertMidpoint(pts, -1)).toEqual(pts);
  });

  it("never mutates the input", () => {
    const pts = rectToPts(0, 0, 10, 10);
    const copy = pts.map((p) => ({ ...p }));
    insertMidpoint(pts, 1);
    expect(pts).toEqual(copy);
  });
});

describe("removeVertex", () => {
  it("removes the vertex at i when more than the minimum remain", () => {
    const pts = rectToPts(0, 0, 10, 10); // 4 vertices
    const next = removeVertex(pts, 1);
    expect(next).toHaveLength(3);
    expect(next).not.toContainEqual(pts[1]);
  });

  it("refuses to drop below the minimum (3) vertices", () => {
    const tri: Point[] = [
      { x: 0, y: 0 },
      { x: 10, y: 0 },
      { x: 5, y: 10 },
    ];
    expect(removeVertex(tri, 0)).toEqual(tri);
    expect(removeVertex(tri, 0)).toHaveLength(MIN_REGION_VERTICES);
  });

  it("returns a copy unchanged for an out-of-range index", () => {
    const pts = rectToPts(0, 0, 10, 10);
    expect(removeVertex(pts, 99)).toEqual(pts);
  });

  it("never mutates the input", () => {
    const pts = rectToPts(0, 0, 10, 10);
    const copy = pts.map((p) => ({ ...p }));
    removeVertex(pts, 2);
    expect(pts).toEqual(copy);
  });
});

describe("pointInPolygon", () => {
  const square = rectToPts(0, 0, 100, 100);

  it("is true for an interior point", () => {
    expect(pointInPolygon({ x: 50, y: 50 }, square)).toBe(true);
  });

  it("is false for an exterior point", () => {
    expect(pointInPolygon({ x: 150, y: 50 }, square)).toBe(false);
    expect(pointInPolygon({ x: -1, y: 50 }, square)).toBe(false);
  });

  it("is false for a degenerate polygon (<3 vertices)", () => {
    expect(pointInPolygon({ x: 0, y: 0 }, [{ x: 0, y: 0 }])).toBe(false);
  });

  it("handles a concave polygon (point in the notch is outside)", () => {
    // An arrow-ish concave shape with a notch around x=50.
    const concave: Point[] = [
      { x: 0, y: 0 },
      { x: 100, y: 0 },
      { x: 100, y: 100 },
      { x: 50, y: 40 },
      { x: 0, y: 100 },
    ];
    expect(pointInPolygon({ x: 50, y: 90 }, concave)).toBe(false); // in the notch
    expect(pointInPolygon({ x: 50, y: 20 }, concave)).toBe(true); // solid area
  });
});

describe("polygonIntersectsRect", () => {
  const region = rectToPts(50, 50, 100, 100); // covers 50..150

  it("hits a node whose rect overlaps the region", () => {
    expect(polygonIntersectsRect(region, rect(100, 100, 80, 80))).toBe(true);
  });

  it("hits a node fully INSIDE the region", () => {
    expect(polygonIntersectsRect(region, rect(60, 60, 20, 20))).toBe(true);
  });

  it("hits a region fully inside a big node (region vertices inside rect)", () => {
    expect(polygonIntersectsRect(region, rect(0, 0, 400, 400))).toBe(true);
  });

  it("hits a node the region only SLICES (edge crossing, no contained corner)", () => {
    // A thin horizontal region crossing a tall node without containing its corners.
    const slice: Point[] = [
      { x: 0, y: 90 },
      { x: 300, y: 90 },
      { x: 300, y: 110 },
      { x: 0, y: 110 },
    ];
    expect(polygonIntersectsRect(slice, rect(100, 0, 50, 300))).toBe(true);
  });

  it("misses a node entirely outside the region", () => {
    expect(polygonIntersectsRect(region, rect(400, 400, 50, 50))).toBe(false);
  });

  it("is false for a degenerate polygon", () => {
    expect(polygonIntersectsRect([{ x: 0, y: 0 }], rect(0, 0, 10, 10))).toBe(
      false,
    );
  });
});

describe("world<->screen helpers", () => {
  const pan = { x: 40, y: 24 };
  const zoom = 0.85;

  it("worldToScreen and screenToWorld round-trip", () => {
    const w: Point = { x: 137.5, y: 60 };
    const s = worldToScreen(w, pan, zoom);
    const back = screenToWorld(s, pan, zoom);
    expect(back.x).toBeCloseTo(w.x, 6);
    expect(back.y).toBeCloseTo(w.y, 6);
  });

  it("worldToScreen applies pan + zoom", () => {
    expect(worldToScreen({ x: 0, y: 0 }, pan, zoom)).toEqual({ x: 40, y: 24 });
    expect(worldToScreen({ x: 100, y: 0 }, pan, zoom)).toEqual({
      x: 40 + 85,
      y: 24,
    });
  });
});
