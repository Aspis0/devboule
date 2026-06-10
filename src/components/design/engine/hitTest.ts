// Topmost-by-z hit testing. PURE geometry — no DOM, no clock, no random. The
// drag layer maps a pointer event to canvas coords then asks which node was hit.

import type { NodeRect, Point } from "../../../types/design";

/** Inclusive point-in-rect test (edges count as inside). */
function contains(p: Point, r: NodeRect): boolean {
  return p.x >= r.x && p.x <= r.x + r.w && p.y >= r.y && p.y <= r.y + r.h;
}

/**
 * Return the topmost node (highest `z`) whose rect contains `point`, or `null` if
 * none does. Deterministic: among rects sharing the max z, the LAST one in
 * iteration order wins (callers pass rects in paint order, so the last-declared
 * is painted on top). Never mutates inputs.
 */
export function hitTest(point: Point, rects: NodeRect[]): NodeRect | null {
  let best: NodeRect | null = null;
  for (const r of rects) {
    if (!contains(point, r)) continue;
    // `>=` so a later rect with an equal z replaces an earlier one (last wins).
    if (best === null || r.z >= best.z) best = r;
  }
  return best;
}
