// PURE drag/resize commit math for the direct-DOM canvas. No DOM, no clock, no
// random. Migrated verbatim (behaviour-preserving) from the retired iframe
// `useDrag.ts` so the existing `dragMath.test.ts` parity assertions still hold —
// the canvas now mutates host style via refs during a gesture and calls THIS to
// compute the single pointer-up commit through the engine (snap + smartGuides +
// manifestOps).

import type {
  DesignManifest,
  DesignNodePlacement,
  NodeRect,
} from "../../../types/design";
import { resizeNode, setPos } from "../engine/manifestOps";
import { smartGuides, snapToGrid } from "../engine/snap";

/** A drag mode: moving the whole node or resizing its width/height. */
export type DragMode = "move" | "resize";

/** Inputs the pure commit computation needs. */
export interface DragCommitInput {
  manifest: DesignManifest;
  id: string;
  mode: DragMode;
  /** Raw pointer delta in WORLD pixels since drag start (already divided by zoom). */
  dx: number;
  dy: number;
  /** Grid size for snapping (<=0 disables grid snap). */
  grid: number;
  /** Resolved rects of the OTHER nodes for smart guides (move mode). */
  others: NodeRect[];
}

/**
 * PURE: compute the manifest resulting from a finished drag/resize. Move applies
 * grid snap + smart-guide alignment to the new top-left; resize sets a new width
 * (and a numeric height when dragged vertically). Returns the SAME manifest
 * reference when the id is absent. No DOM, no clock, no random.
 */
export function computeDragCommit(input: DragCommitInput): DesignManifest {
  const node = input.manifest.nodes[input.id];
  if (!node) return input.manifest;

  if (input.mode === "resize") {
    const newW = Math.max(1, snapToGrid(node.w + input.dx, input.grid));
    // Only pin a numeric height when the user actually dragged vertically AND the
    // node already had (or now gets) a fixed height; a pure-horizontal resize
    // keeps `h` as-is (auto stays auto).
    if (input.dy !== 0) {
      const baseH = typeof node.h === "number" ? node.h : 0;
      const newH = Math.max(1, snapToGrid(baseH + input.dy, input.grid));
      return resizeNode(input.manifest, input.id, newW, newH);
    }
    return resizeNode(input.manifest, input.id, newW);
  }

  // move: grid-snap the raw new position, then nudge by smart-guide delta.
  const snappedX = snapToGrid(node.x + input.dx, input.grid);
  const snappedY = snapToGrid(node.y + input.dy, input.grid);
  const movingRect: NodeRect = {
    id: input.id,
    x: snappedX,
    y: snappedY,
    w: node.w,
    h: typeof node.h === "number" ? node.h : 0,
    z: node.z,
  };
  const guides = smartGuides(movingRect, input.others);
  return setPos(input.manifest, input.id, snappedX + guides.dx, snappedY + guides.dy);
}

/**
 * Resolve the rects of every node EXCEPT `excludeId` for smart guides. Pure.
 * `measuredH` (optional) supplies a measured px height per id so an `"auto"`
 * node aligns by its REAL rendered height; ids absent from it resolve "auto" to 0
 * (the iframe-era behaviour the parity tests assert).
 */
export function otherRects(
  manifest: DesignManifest,
  excludeId: string,
  measuredH?: Record<string, number>,
): NodeRect[] {
  const out: NodeRect[] = [];
  for (const [id, p] of Object.entries(manifest.nodes)) {
    if (id === excludeId) continue;
    if (p.hidden) continue; // never align against a hidden layer
    const h =
      typeof p.h === "number" ? p.h : measuredH?.[id] ?? 0;
    out.push({ id, x: p.x, y: p.y, w: p.w, h, z: p.z });
  }
  return out;
}

/** A live placement preview while dragging (applied to the host's inline style
 *  via a ref only). Pure helper so the gesture loop has no branching of its own. */
export function previewPlacement(
  base: DesignNodePlacement,
  mode: DragMode,
  dx: number,
  dy: number,
): DesignNodePlacement {
  if (mode === "resize") {
    const w = Math.max(1, base.w + dx);
    const h =
      dy !== 0 && typeof base.h === "number" ? Math.max(1, base.h + dy) : base.h;
    return { ...base, w, h };
  }
  return { ...base, x: base.x + dx, y: base.y + dy };
}
