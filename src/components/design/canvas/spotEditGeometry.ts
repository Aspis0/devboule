// PURE geometry for Spot Edit (the AI region tool). No DOM, no clock, no random —
// every function is a deterministic, unit-testable transform on plain points/rects.
// Ported from the prototype's `canvas.jsx` region helpers (rectToPts/bboxOf/midpoint/
// vertex ops) plus the node-vs-polygon hit test and world<->screen helpers (which
// reuse `viewportMath`). The React `SpotEdit` overlay owns the ephemeral region
// STATE; this module owns the math so the behaviour is testable without a browser.

import type { NodeRect, Point } from "../../../types/design";
import {
  screenToWorld as vmScreenToWorld,
  worldToScreen as vmWorldToScreen,
  type Pan,
} from "./viewportMath";

/** Minimum vertices a region polygon may have (a triangle; below this it is not a
 *  closed area). `removeVertex` refuses to drop below this. */
export const MIN_REGION_VERTICES = 3;

/** Build the 4 corner points (clockwise from top-left) of an axis-aligned rect.
 *  Mirrors the prototype's `rectToPts`. PURE. */
export function rectToPts(x: number, y: number, w: number, h: number): Point[] {
  return [
    { x, y },
    { x: x + w, y },
    { x: x + w, y: y + h },
    { x, y: y + h },
  ];
}

/** Axis-aligned bounding box of a set of points. Mirrors the prototype's `bboxOf`.
 *  Returns a zero box at the origin for an empty input (defensive — callers always
 *  pass >=3 points). PURE. */
export function bboxOf(pts: Point[]): { x: number; y: number; w: number; h: number } {
  if (pts.length === 0) return { x: 0, y: 0, w: 0, h: 0 };
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const p of pts) {
    if (p.x < minX) minX = p.x;
    if (p.y < minY) minY = p.y;
    if (p.x > maxX) maxX = p.x;
    if (p.y > maxY) maxY = p.y;
  }
  return { x: minX, y: minY, w: maxX - minX, h: maxY - minY };
}

/**
 * Insert a NEW vertex at the midpoint of the edge from `pts[i]` to `pts[(i+1)%n]`,
 * returning a NEW point array with the midpoint spliced in AFTER index `i`. The
 * caller then typically drags that new vertex (its index is `i + 1`). Mirrors the
 * prototype's `startMidDrag` splice. PURE — never mutates the input. An out-of-range
 * `i` returns the input unchanged.
 */
export function insertMidpoint(pts: Point[], i: number): Point[] {
  if (i < 0 || i >= pts.length) return pts.slice();
  const p = pts[i];
  const q = pts[(i + 1) % pts.length];
  const mid: Point = { x: (p.x + q.x) / 2, y: (p.y + q.y) / 2 };
  const newIdx = i + 1;
  return [...pts.slice(0, newIdx), mid, ...pts.slice(newIdx)];
}

/**
 * Remove the vertex at index `i`, returning a NEW array — but ONLY when more than
 * {@link MIN_REGION_VERTICES} remain (a polygon must keep at least 3 vertices). When
 * removal would drop below the minimum, or `i` is out of range, the input is
 * returned UNCHANGED. Mirrors the prototype's double-click vertex removal guard.
 * PURE.
 */
export function removeVertex(pts: Point[], i: number): Point[] {
  if (i < 0 || i >= pts.length) return pts.slice();
  if (pts.length <= MIN_REGION_VERTICES) return pts.slice();
  return pts.filter((_, j) => j !== i);
}

/**
 * Even-odd ray-cast point-in-polygon test. Returns true when `point` lies strictly
 * inside the polygon described by `pts` (edge/vertex membership is unspecified — the
 * standard ray-cast convention). PURE. Used for region hit-testing helpers.
 */
export function pointInPolygon(point: Point, pts: Point[]): boolean {
  if (pts.length < 3) return false;
  let inside = false;
  for (let i = 0, j = pts.length - 1; i < pts.length; j = i++) {
    const xi = pts[i].x;
    const yi = pts[i].y;
    const xj = pts[j].x;
    const yj = pts[j].y;
    const intersect =
      yi > point.y !== yj > point.y &&
      point.x < ((xj - xi) * (point.y - yi)) / (yj - yi) + xi;
    if (intersect) inside = !inside;
  }
  return inside;
}

/** Segment-vs-segment proper-or-improper intersection test (helper for
 *  {@link polygonIntersectsRect}). PURE. */
function segmentsIntersect(
  a1: Point,
  a2: Point,
  b1: Point,
  b2: Point,
): boolean {
  const d = (p: Point, q: Point, r: Point) =>
    (q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x);
  const onSeg = (p: Point, q: Point, r: Point) =>
    Math.min(p.x, r.x) <= q.x &&
    q.x <= Math.max(p.x, r.x) &&
    Math.min(p.y, r.y) <= q.y &&
    q.y <= Math.max(p.y, r.y);
  const d1 = d(b1, b2, a1);
  const d2 = d(b1, b2, a2);
  const d3 = d(a1, a2, b1);
  const d4 = d(a1, a2, b2);
  if (
    ((d1 > 0 && d2 < 0) || (d1 < 0 && d2 > 0)) &&
    ((d3 > 0 && d4 < 0) || (d3 < 0 && d4 > 0))
  ) {
    return true;
  }
  // Collinear-overlap cases.
  if (d1 === 0 && onSeg(b1, a1, b2)) return true;
  if (d2 === 0 && onSeg(b1, a2, b2)) return true;
  if (d3 === 0 && onSeg(a1, b1, a2)) return true;
  if (d4 === 0 && onSeg(a1, b2, a2)) return true;
  return false;
}

/**
 * True when the polygon `pts` overlaps the axis-aligned `rect` (a node). Robust
 * overlap (not just bbox): true if ANY rect corner is inside the polygon, OR any
 * polygon vertex is inside the rect, OR any polygon edge crosses any rect edge.
 * This catches a region that slices a node without containing a corner, and a node
 * that fully contains the region. PURE — used by `onRegionAnalyze` to pick hit nodes.
 *
 * NOTE: the prototype used a looser BBOX-vs-rect overlap. This is a STRICTER,
 * correct polygon test; the bbox case is a subset of it, so it never reports FEWER
 * hits than the prototype for a rectangular region and is more accurate for a
 * reshaped (non-rectangular) one.
 */
export function polygonIntersectsRect(pts: Point[], rect: NodeRect): boolean {
  if (pts.length < 3) return false;
  const corners: Point[] = [
    { x: rect.x, y: rect.y },
    { x: rect.x + rect.w, y: rect.y },
    { x: rect.x + rect.w, y: rect.y + rect.h },
    { x: rect.x, y: rect.y + rect.h },
  ];
  // 1) Any rect corner inside the polygon.
  for (const c of corners) if (pointInPolygon(c, pts)) return true;
  // 2) Any polygon vertex inside the rect.
  for (const p of pts) {
    if (
      p.x >= rect.x &&
      p.x <= rect.x + rect.w &&
      p.y >= rect.y &&
      p.y <= rect.y + rect.h
    ) {
      return true;
    }
  }
  // 3) Any polygon edge crosses any rect edge.
  for (let i = 0; i < pts.length; i++) {
    const a1 = pts[i];
    const a2 = pts[(i + 1) % pts.length];
    for (let c = 0; c < corners.length; c++) {
      const b1 = corners[c];
      const b2 = corners[(c + 1) % corners.length];
      if (segmentsIntersect(a1, a2, b1, b2)) return true;
    }
  }
  return false;
}

/** World point -> screen (viewport-relative) point. Thin re-export of viewportMath
 *  so the SpotEdit overlay imports its transforms from one place. PURE. */
export function worldToScreen(
  p: Point,
  pan: Pan,
  zoom: number,
): Point {
  return vmWorldToScreen(p.x, p.y, pan, zoom);
}

/** Screen (viewport-relative) point -> world point. Thin re-export of viewportMath.
 *  PURE. */
export function screenToWorld(
  p: Point,
  pan: Pan,
  zoom: number,
): Point {
  return vmScreenToWorld(p.x, p.y, pan, zoom);
}
