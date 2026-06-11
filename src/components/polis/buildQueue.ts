// buildQueue.ts — VIEWPORT-PRIORITIZED build ordering for the Polis chunked build
// (Phase B2b).
//
// The chunked build (PolisRenderer.setCityState → runBatch) places every building
// across requestAnimationFrame batches. B2a/earlier built them in pure DEPTH order,
// so time-to-visible-city scaled with the WHOLE city: the chunks the camera is
// looking at might be placed last. B2b reorders the work so the buildings the
// VIEWPORT can see (plus a configurable preload ring of chunks around it) build
// FIRST — depth-sorted within — and the rest fill in afterwards in distance order.
// The map becomes interactive as soon as the visible set is placed.
//
// This module is the PURE, PIXI-free core of that ordering: it never touches the
// viewport or PIXI; the renderer supplies each item's chunk grid coords, a
// predicate for "is this chunk in the visible+ring priority region", and the
// center chunk used to break ties by distance. The renderer keeps depth ordering
// by passing items ALREADY depth-sorted — this is a STABLE partition, so depth
// order is preserved inside each priority bucket.
//
// Reprioritization (camera moves mid-fill) reuses the SAME function on the
// not-yet-placed remainder, with a fresh predicate/center — no rebuild of placed
// chunks, just a re-sort of the pending queue.

/** A build item's chunk grid coordinates (cx = floor(tileX/CHUNK_SIZE), etc.) and
 *  its STABLE depth rank (the index in the depth-sorted source array). Only these
 *  are needed to order the build; the renderer maps the result back to buildings. */
export interface BuildQueueItem {
  /** Chunk column (floor(tileX / CHUNK_SIZE)). */
  cx: number;
  /** Chunk row (floor(tileY / CHUNK_SIZE)). */
  cy: number;
}

/**
 * Order `items` (assumed ALREADY depth-sorted) so the priority region builds
 * first. PURE + STABLE.
 *
 * @param items      build items in DEPTH order; index i is the source rank.
 * @param isPriority `(cx, cy) => boolean` — true iff the chunk is inside the
 *                   viewport + preload-ring region (the renderer computes this from
 *                   the visible bounds + ring). Priority items keep their depth
 *                   order (stable).
 * @param center     the reference chunk `(cx, cy)` (typically the viewport-center
 *                   chunk). NON-priority items are ordered by Chebyshev distance to
 *                   it (nearest first), ties broken by depth rank — so the fill
 *                   spreads outward from the camera.
 *
 * @returns an array of INDICES into `items`, covering [0, items.length) exactly
 *          once: every priority item (in depth order) first, then the rest by
 *          distance-then-depth. The renderer consumes this order in its batches.
 */
export function orderBuildQueue(
  items: readonly BuildQueueItem[],
  isPriority: (cx: number, cy: number) => boolean,
  center: { cx: number; cy: number },
): number[] {
  const priority: number[] = [];
  // Non-priority entries carry their distance + depth rank for the secondary sort.
  const rest: { idx: number; dist: number }[] = [];

  for (let i = 0; i < items.length; i++) {
    const it = items[i];
    if (isPriority(it.cx, it.cy)) {
      // Stable: pushed in source (depth) order, never re-sorted.
      priority.push(i);
    } else {
      // Chebyshev (chunk-grid) distance — the natural "rings around the camera"
      // metric for a square chunk grid.
      const dist = Math.max(
        Math.abs(it.cx - center.cx),
        Math.abs(it.cy - center.cy),
      );
      rest.push({ idx: i, dist });
    }
  }

  // Distance-nearest first; ties keep depth order (idx ascending) → deterministic.
  rest.sort((a, b) => (a.dist - b.dist) || (a.idx - b.idx));

  const out = priority.slice();
  for (const r of rest) out.push(r.idx);
  return out;
}

/**
 * Build the predicate {@link orderBuildQueue} needs from a set of priority chunk
 * KEYS (the `"cx,cy"` strings the renderer already keys chunks by). Returned as a
 * closure so the renderer can compute the visible+ring key set once (from the
 * viewport bounds) and reuse it for both the initial order and a reprioritization.
 */
export function priorityFromKeys(
  keys: ReadonlySet<string>,
): (cx: number, cy: number) => boolean {
  return (cx, cy) => keys.has(`${cx},${cy}`);
}

/**
 * Expand a set of base chunk keys (the chunks the viewport rectangle intersects)
 * by `ring` chunks in every direction (including diagonals), returning the full
 * priority key set. PURE — operates only on the `"cx,cy"` key strings. `ring <= 0`
 * returns the base set unchanged (a copy). Used by the renderer to turn the
 * visible chunks + the profile's preload ring into the priority region.
 */
export function expandChunkRing(
  baseKeys: ReadonlySet<string>,
  ring: number,
): Set<string> {
  if (ring <= 0) return new Set(baseKeys);
  const out = new Set<string>();
  for (const key of baseKeys) {
    const comma = key.indexOf(",");
    const cx = Number(key.slice(0, comma));
    const cy = Number(key.slice(comma + 1));
    if (!Number.isFinite(cx) || !Number.isFinite(cy)) continue;
    for (let dx = -ring; dx <= ring; dx++) {
      for (let dy = -ring; dy <= ring; dy++) {
        out.add(`${cx + dx},${cy + dy}`);
      }
    }
  }
  return out;
}
