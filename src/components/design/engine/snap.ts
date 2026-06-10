// Snap-to-grid + smart alignment guides. PURE geometry — no DOM, no clock, no
// random. Part of the deterministic reconciliation core (LOCKED architecture
// 1.5). The DOM drag layer feeds in plain rects and applies the returned delta.

import type { NodeRect } from "../../../types/design";

/** Default pixel threshold within which an edge/center is considered "aligned". */
export const DEFAULT_GUIDE_THRESHOLD = 5;

/**
 * Snap a single scalar to the nearest multiple of `grid`. A non-positive grid
 * disables snapping (returns the value unchanged) so the caller can turn grid
 * snapping off by passing `0`. Deterministic (`Math.round` half-up).
 */
export function snapToGrid(value: number, grid: number): number {
  if (!(grid > 0)) return value; // also handles NaN grid
  return Math.round(value / grid) * grid;
}

/** A single alignment guide line produced by `smartGuides`. */
export interface AlignmentGuide {
  orientation: "vertical" | "horizontal";
  /** Canvas coordinate of the guide line (x for vertical, y for horizontal). */
  position: number;
}

/** Result of a smart-guide computation: the guide lines to draw + the snap delta. */
export interface SmartGuideResult {
  guides: AlignmentGuide[];
  /** Suggested x correction to apply to the moving rect (0 if none). */
  dx: number;
  /** Suggested y correction to apply to the moving rect (0 if none). */
  dy: number;
}

/** Candidate alignment coordinates an axis can snap to, derived from one rect. */
function xAnchors(r: NodeRect): number[] {
  return [r.x, r.x + r.w / 2, r.x + r.w]; // left, center, right
}
function yAnchors(r: NodeRect): number[] {
  return [r.y, r.y + r.h / 2, r.y + r.h]; // top, middle, bottom
}

/**
 * Compute alignment guides for `moving` against `others`. For each axis we test
 * the moving rect's three anchors (edges + center) against every other rect's
 * three anchors and keep the CLOSEST pair within `threshold`. Returns the snap
 * delta (`dx`/`dy`) plus the guide lines to render at the aligned coordinate.
 *
 * Deterministic and total: the moving node is excluded from `others` by id; ties
 * resolve to the first-seen candidate (stable iteration order). Never mutates
 * inputs.
 */
export function smartGuides(
  moving: NodeRect,
  others: NodeRect[],
  threshold: number = DEFAULT_GUIDE_THRESHOLD,
): SmartGuideResult {
  const movingX = xAnchors(moving);
  const movingY = yAnchors(moving);

  let bestX: { delta: number; position: number; dist: number } | null = null;
  let bestY: { delta: number; position: number; dist: number } | null = null;

  for (const other of others) {
    if (other.id === moving.id) continue; // never align against self
    const otherX = xAnchors(other);
    const otherY = yAnchors(other);

    for (const ma of movingX) {
      for (const oa of otherX) {
        const dist = Math.abs(oa - ma);
        if (dist <= threshold && (bestX === null || dist < bestX.dist)) {
          bestX = { delta: oa - ma, position: oa, dist };
        }
      }
    }
    for (const ma of movingY) {
      for (const oa of otherY) {
        const dist = Math.abs(oa - ma);
        if (dist <= threshold && (bestY === null || dist < bestY.dist)) {
          bestY = { delta: oa - ma, position: oa, dist };
        }
      }
    }
  }

  const guides: AlignmentGuide[] = [];
  if (bestX) guides.push({ orientation: "vertical", position: bestX.position });
  if (bestY) guides.push({ orientation: "horizontal", position: bestY.position });

  return {
    guides,
    dx: bestX ? bestX.delta : 0,
    dy: bestY ? bestY.delta : 0,
  };
}
