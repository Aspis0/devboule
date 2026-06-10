// Pointer-based drag/resize for canvas hosts. THIN DOM/React layer: during a drag
// it mutates ONLY the host element's inline `style` inside the iframe
// `contentDocument` (no React re-render); on pointer-up it computes the NEW
// manifest via the PURE engine (snap + smartGuides + manifestOps) and commits
// once. All decision math is the engine's; this hook only routes events.

import { useCallback, useEffect, useRef } from "react";
import type {
  DesignManifest,
  DesignNodePlacement,
  NodeRect,
} from "../../types/design";
import { bringToFront, resizeNode, setPos } from "./engine/manifestOps";
import { smartGuides, snapToGrid } from "./engine/snap";
import { applyPlacement, CANVAS_ROOT_ID, NODE_ID_ATTR } from "./iframeInject";

/** A drag mode: moving the whole node or resizing its width/height. */
export type DragMode = "move" | "resize";

/** Inputs the pure commit computation needs. */
export interface DragCommitInput {
  manifest: DesignManifest;
  id: string;
  mode: DragMode;
  /** Raw pointer delta in canvas pixels since drag start. */
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

/** A live placement preview while dragging (applied to the DOM only). */
function previewPlacement(
  base: DesignNodePlacement,
  mode: DragMode,
  dx: number,
  dy: number,
): DesignNodePlacement {
  if (mode === "resize") {
    const w = Math.max(1, base.w + dx);
    const h =
      dy !== 0 && typeof base.h === "number"
        ? Math.max(1, base.h + dy)
        : base.h;
    return { ...base, w, h };
  }
  return { ...base, x: base.x + dx, y: base.y + dy };
}

export interface UseDragOptions {
  /** Returns the live iframe document (or null until loaded). */
  getDoc: () => Document | null;
  /** Returns the current manifest (read fresh — never captured stale). */
  getManifest: () => DesignManifest;
  /** Canvas grid for snapping. */
  grid: number;
  /** Commit a new manifest after a drag/resize finishes. */
  onCommit: (next: DesignManifest) => void;
}

/**
 * Hook returning a `beginDrag` starter to wire to a host's pointerdown. It
 * captures the pointer, live-updates the host style during move, and commits via
 * the pure engine on pointer-up. Listeners are always cleaned up (pointerup +
 * an effect teardown) so a drag in flight at unmount cannot leak.
 */
export function useDrag(opts: UseDragOptions) {
  // Keep the latest options in a ref so the long-lived window listeners never
  // capture a stale closure (manifest/grid/onCommit can change between renders).
  const optsRef = useRef(opts);
  optsRef.current = opts;

  // Active drag state (null when idle). Held in a ref: a drag must not trigger
  // React re-renders (that would defeat the live-DOM-only design). `doc` and
  // `host` are the EXACT document/element the pointerdown originated in, so
  // pointermove/up are listened on the SAME surface — clientX/Y stay in one
  // coordinate space and the deltas are correct (the iframe is same-origin).
  const drag = useRef<{
    id: string;
    mode: DragMode;
    startX: number;
    startY: number;
    // The (possibly z-raised) placement used for the DOM preview math.
    base: DesignNodePlacement;
    // B1 — the node's placement AS COMMITTED IN THE MANIFEST at drag start (BEFORE
    // the DOM-only bring-to-front z-bump). The pointer-up guard compares the fresh
    // manifest's node against THIS to detect a mid-drag manifest swap (generation /
    // self-repair) and abort rather than teleport. Must be the un-raised value so a
    // legit drag of a non-top node isn't spuriously aborted on the z difference.
    committedBase: DesignNodePlacement;
    doc: Document;
    host: Element;
    pointerId: number;
    // True when the node was visually raised on drag-start; the SINGLE pointer-up
    // commit then folds the bring-to-front z-bump into the position commit so a
    // drag produces exactly one manifest commit (not one on start + one on up).
    raised: boolean;
  } | null>(null);

  const onMove = useCallback((e: PointerEvent) => {
    const d = drag.current;
    if (!d) return;
    const root = d.doc.getElementById(CANVAS_ROOT_ID);
    if (!root) return;
    const dx = e.clientX - d.startX;
    const dy = e.clientY - d.startY;
    applyPlacement(root, d.id, previewPlacement(d.base, d.mode, dx, dy));
  }, []);

  // `onUp` references `detach` and `detach` references `onUp` — a genuine cycle.
  // We break it (and the dep-array TDZ that would otherwise force a dishonest
  // `[]`) with a forward ref to `onUp`, populated after both are defined. `detach`
  // can then list its real, stable deps honestly.
  const onUpRef = useRef<(e: PointerEvent) => void>(() => {});

  const detach = useCallback(
    (d: NonNullable<typeof drag.current>) => {
      const up = onUpRef.current;
      d.doc.removeEventListener("pointermove", onMove);
      d.doc.removeEventListener("pointerup", up);
      d.doc.removeEventListener("pointercancel", up);
      try {
        (d.host as HTMLElement).releasePointerCapture?.(d.pointerId);
      } catch {
        // capture may already be gone (host removed / pointer lost) — ignore.
      }
    },
    [onMove],
  );

  const onUp = useCallback(
    (e: PointerEvent) => {
      const d = drag.current;
      if (!d) return;
      drag.current = null;
      detach(d);
      const dx = e.clientX - d.startX;
      const dy = e.clientY - d.startY;
      const o = optsRef.current;
      const fresh = o.getManifest();
      // B1 — STALE-MANIFEST GUARD. The delta was measured against `d.base`
      // (the placement captured at drag start). If a generation / self-repair
      // committed a NEW manifest under the drag, the live node may be gone or sit
      // at a different base; computing against that live node would teleport it
      // (or silently drop the node). So: if the dragged id is no longer present,
      // OR its current placement no longer matches the base we captured, ABORT
      // the commit entirely — no write — rather than persisting a stale/teleported
      // position. The in-flight DOM preview is discarded on the next React render.
      const live = fresh.nodes[d.id];
      if (!live || !samePlacement(live, d.committedBase)) return;
      // Fold the bring-to-front (applied only to the DOM on drag-start) into THIS
      // single commit so the whole drag yields exactly one manifest write. Safe to
      // do now: we proved the node still exists with the expected base placement.
      const manifest = d.raised ? bringToFront(fresh, d.id) : fresh;
      const others = otherRects(manifest, d.id);
      const next = computeDragCommit({
        manifest,
        id: d.id,
        mode: d.mode,
        dx,
        dy,
        grid: o.grid,
        others,
      });
      // Commit when EITHER the position/size changed OR the z was raised.
      if (next !== fresh) o.onCommit(next);
    },
    [detach],
  );
  // Keep the forward ref pointing at the live `onUp` so `detach` removes the
  // EXACT listener instance that `beginDrag` added.
  onUpRef.current = onUp;

  const beginDrag = useCallback(
    (id: string, mode: DragMode, e: PointerEvent) => {
      const o = optsRef.current;
      const manifest = o.getManifest();
      const base = manifest.nodes[id];
      if (!base) return;
      const host = e.target instanceof Element ? e.target.closest(`[data-node-id]`) : null;
      const doc = host?.ownerDocument ?? optsRef.current.getDoc();
      if (!host || !doc) return;
      // Bring-to-front on drag start (LOCKED 1.5) so the dragged node is on top —
      // but apply the raised z to the DOM ONLY (no manifest commit here). The
      // single pointer-up commit folds the z-bump in, so a drag = one commit.
      const raisedManifest = bringToFront(manifest, id);
      const raised = raisedManifest !== manifest;
      const raisedNode = raisedManifest.nodes[id] ?? base;
      if (raised) applyPlacement(host.parentElement ?? host, id, raisedNode);
      drag.current = {
        id,
        mode,
        startX: e.clientX,
        startY: e.clientY,
        base: raisedNode,
        // Snapshot of the committed placement (pre-raise) for the B1 staleness
        // guard at pointer-up. `base` (above) is the raised value for DOM preview.
        committedBase: base,
        doc,
        host,
        pointerId: e.pointerId,
        raised,
      };
      // Pointer capture keeps events flowing to the host even if the pointer
      // leaves its bounds mid-drag (e.g. fast drags).
      try {
        (host as HTMLElement).setPointerCapture?.(e.pointerId);
      } catch {
        // non-fatal: fall back to plain document listeners below.
      }
      doc.addEventListener("pointermove", onMove);
      doc.addEventListener("pointerup", onUp);
      doc.addEventListener("pointercancel", onUp);
    },
    [onMove, onUp],
  );

  // Teardown: if the component unmounts mid-drag, drop the listeners on the
  // captured document so a stale handler can never fire against an unmounted tree.
  useEffect(() => {
    return () => {
      const d = drag.current;
      if (d) {
        drag.current = null;
        detach(d);
      }
    };
  }, [detach]);

  return { beginDrag };
}

/**
 * B1 — true when two placements are positionally identical (x/y/w/h/z). Used by
 * the pointer-up staleness guard to detect that the manifest was replaced under a
 * live drag (a generation/self-repair commit). `h` may be a number or "auto"; we
 * compare it with strict equality (number===number or "auto"==="auto"). Pure.
 */
export function samePlacement(
  a: DesignNodePlacement,
  b: DesignNodePlacement,
): boolean {
  return (
    a.x === b.x &&
    a.y === b.y &&
    a.w === b.w &&
    a.h === b.h &&
    a.z === b.z
  );
}

/** Resolve the rects of every node EXCEPT `excludeId` for smart guides. Pure. */
export function otherRects(
  manifest: DesignManifest,
  excludeId: string,
): NodeRect[] {
  const out: NodeRect[] = [];
  for (const [id, p] of Object.entries(manifest.nodes)) {
    if (id === excludeId) continue;
    out.push({
      id,
      x: p.x,
      y: p.y,
      w: p.w,
      h: typeof p.h === "number" ? p.h : 0,
      z: p.z,
    });
  }
  return out;
}

// Re-export the marker so a consumer can locate hosts without importing two files.
export { NODE_ID_ATTR };
