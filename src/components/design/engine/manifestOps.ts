// Immutable operations on a design manifest's node map. PURE: no DOM, no PIXI,
// no React, no clock/random — every function returns a NEW manifest (or the
// SAME reference when the op is a no-op) and never mutates its inputs. This is
// the deterministic placement core (LOCKED architecture 1.5); the DOM layer only
// translates pointer events into calls here.

import type {
  DesignManifest,
  DesignNodeHeight,
  DesignNodePlacement,
} from "../../../types/design";

/** Shallow-clone the manifest with a replaced node entry (structural sharing of
 * every other node). Returns the SAME manifest when `id` is absent so callers can
 * cheaply detect a no-op by reference equality. */
function withNode(
  manifest: DesignManifest,
  id: string,
  update: (node: DesignNodePlacement) => DesignNodePlacement,
): DesignManifest {
  const existing = manifest.nodes[id];
  if (!existing) return manifest;
  const nextNode = update(existing);
  // If the update produced a structurally identical node, still return a new
  // manifest object only when something actually changed. We compare by the
  // produced object identity: `update` always builds a fresh object, so we treat
  // it as changed. Callers relying on no-op identity use the missing-id path.
  return {
    ...manifest,
    nodes: { ...manifest.nodes, [id]: nextNode },
  };
}

/** Move a node by a relative delta. No-op (same reference) if `id` is absent. */
export function moveNode(
  manifest: DesignManifest,
  id: string,
  dx: number,
  dy: number,
): DesignManifest {
  return withNode(manifest, id, (n) => ({ ...n, x: n.x + dx, y: n.y + dy }));
}

/** Set a node's absolute position. No-op (same reference) if `id` is absent. */
export function setPos(
  manifest: DesignManifest,
  id: string,
  x: number,
  y: number,
): DesignManifest {
  return withNode(manifest, id, (n) => ({ ...n, x, y }));
}

/**
 * Resize a node: always sets `w`. `h` semantics:
 *  - omitted (`undefined`) → keep the node's existing height (numeric or "auto"),
 *  - a number → pin a fixed numeric height,
 *  - "auto" → revert to hug-contents.
 * No-op (same reference) if `id` is absent.
 */
export function resizeNode(
  manifest: DesignManifest,
  id: string,
  w: number,
  h?: DesignNodeHeight,
): DesignManifest {
  return withNode(manifest, id, (n) => ({
    ...n,
    w,
    h: h === undefined ? n.h : h,
  }));
}

/** Bring a node to the front: `z = max(z over all nodes) + 1`. Deterministic. */
export function bringToFront(
  manifest: DesignManifest,
  id: string,
): DesignManifest {
  if (!manifest.nodes[id]) return manifest;
  let maxZ = -Infinity;
  for (const node of Object.values(manifest.nodes)) {
    if (node.z > maxZ) maxZ = node.z;
  }
  const target = maxZ === -Infinity ? 0 : maxZ + 1;
  return withNode(manifest, id, (n) => ({ ...n, z: target }));
}

/** Send a node to the back: `z = min(z over all nodes) - 1`. Deterministic. */
export function sendToBack(
  manifest: DesignManifest,
  id: string,
): DesignManifest {
  if (!manifest.nodes[id]) return manifest;
  let minZ = Infinity;
  for (const node of Object.values(manifest.nodes)) {
    if (node.z < minZ) minZ = node.z;
  }
  const target = minZ === Infinity ? 0 : minZ - 1;
  return withNode(manifest, id, (n) => ({ ...n, z: target }));
}

/** Clamp an index into `[0, len-1]` (or `[0, len]` for an insertion point). */
function clamp(value: number, lo: number, hi: number): number {
  if (value < lo) return lo;
  if (value > hi) return hi;
  return value;
}

/**
 * Reorder the paint/stacking id list: remove the entry at `from` and re-insert it
 * at `to`. Indices are clamped (never throws). Returns the SAME array reference
 * for an empty list or a true no-op so callers can detect "nothing changed".
 * Never mutates the input array.
 */
export function reorder(order: string[], from: number, to: number): string[] {
  if (order.length === 0) return order;
  const f = clamp(from, 0, order.length - 1);
  const t = clamp(to, 0, order.length - 1);
  if (f === t) return order;
  const next = [...order];
  const [moved] = next.splice(f, 1);
  next.splice(t, 0, moved);
  return next;
}
