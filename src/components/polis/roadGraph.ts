// RoadGraph — building-level navigation graph for the Polis "AgentMover".
//
// PURPOSE: let a citizen (agent) WALK from one building to another along the
// REAL street network instead of teleporting. The backend already routes each
// road as a deterministic world-grid polyline (`Road.path`, cartesian tile
// waypoints, routed around buildings and sharing segments — see Rust
// `grid::route_roads`). This module assembles those polylines into an undirected
// graph whose NODES are building fileIds and whose EDGES are roads, then BFS's a
// route between two buildings and returns the concatenated, correctly-oriented
// ISO waypoint list the AgentLayer animates along.
//
// HONESTY RULE (pure-data): we only ever produce a path made of REAL road
// polylines between REAL buildings. If two buildings are not connected by roads
// (disconnected component, or a building with no incident routed road), there is
// NO honest path — `findRoute` returns null and the caller falls back to a
// fade-teleport. We never invent a waypoint.
//
// DETERMINISM: adjacency is built in a stable order and each node's neighbours
// are sorted by fileId, so the BFS expands nodes in a fixed lexicographic order
// and a given (from, to) always yields the SAME route across runs. A visited cap
// bounds the search on pathological graphs (returns null past the cap).
//
// This mirrors the backend's own grid A* but operates one level up, on the
// building graph, so agent navigation costs a tiny BFS over a few-hundred-node
// graph — computed ONLY when an agent actually changes building, never per frame.

import { cartToIso, type IsoPoint } from "./iso";
import { pathTouchesBlocked } from "./navWalkable";
import type { Road } from "../../types/city";

/** A cheap per-tile "this is blocked water" predicate (see navWalkable.ts). An
 *  edge whose routed polyline touches a blocked tile is rejected so a citizen can
 *  never walk onto open sea / an un-bridged river. Defaults to "nothing blocked"
 *  when no terrain frame is supplied (no behaviour change for callers that don't
 *  pass one — the backend already guarantees road tiles are walkable). */
export type TileBlocked = (gx: number, gy: number) => boolean;

/** Hard cap on BFS node expansions — a route that needs more is treated as
 *  unreachable (returns null). A few-hundred-node building graph never hits
 *  this; the cap only guards against a pathologically large/degenerate city. */
export const ROUTE_VISIT_CAP = 4096;

/** One incident road as seen from a node: the neighbour building and the road's
 *  cartesian waypoint polyline, ALWAYS stored oriented away from `this` node
 *  (i.e. path[0] is at THIS node's building, path[last] at the neighbour). */
interface Edge {
  /** The fileId of the building at the far end of this road. */
  to: string;
  /** Cartesian (tile) waypoints, oriented from THIS node → `to`. >= 2 points. */
  path: { x: number; y: number }[];
  /** The road's import weight (>=1). Used ONLY by the decorative ambient crowd
   *  to bias foot traffic toward busy arterials; navigation/BFS ignores it. */
  weight: number;
}

/**
 * Undirected building-fileId graph with per-edge street polylines.
 *
 * Build ONCE per city (whenever roads change). `findRoute` is then a cheap,
 * pure, deterministic BFS that returns ISO waypoints — or null when there is no
 * honest road path.
 */
export class RoadGraph {
  /** fileId → its incident edges, each sorted by neighbour fileId (stable). */
  private adj = new Map<string, Edge[]>();

  // ---- Per-instance BFS scratch buffers (reused across findRoute calls) ----
  // The old code allocated a fresh Map + Set + string[] queue on EVERY call —
  // one per ambient/agent destination pick. With the higher L1 ambient density
  // that is real GC pressure. These reusable buffers are cleared at the START of
  // each findRoute and never escape it, so a route is still a pure function of
  // (from, to) and runs single-threaded (JS) — no re-entrancy hazard. The
  // reconstruction `edges` buffer is reused too; only the returned `out` list is
  // freshly allocated (the caller owns it).
  private scratchCameFrom = new Map<string, { prev: string; edge: Edge }>();
  private scratchVisited = new Set<string>();
  private scratchQueue: string[] = [];
  private scratchEdges: Edge[] = [];

  // The graph is IMMUTABLE after construction, so the `nodeIds` / `nodeWeights`
  // views never change. Compute each ONCE on first access and cache it — callers
  // (ambient crowd) treat them as read-only (they already do), so handing back
  // the cached array avoids a fresh allocation on every diff/syncAmbient.
  private cachedNodeIds: string[] | null = null;
  private cachedNodeWeights: number[] | null = null;

  /**
   * @param roads     the city's roads (only those with a routed `path` of >= 2
   *                  cartesian waypoints AND distinct from/to form edges; a
   *                  road without a routed polyline cannot be walked honestly
   *                  and is skipped — the caller's fallback handles it).
   * @param isBlocked OPTIONAL defensive guard: a per-tile predicate that is true
   *                  for a tile a citizen must never stand on (open sea / un-
   *                  bridged river). An edge whose routed polyline touches a
   *                  blocked tile is REJECTED (not added to the graph), so a
   *                  citizen can never be routed onto water. Omitted → nothing is
   *                  blocked (the backend already guarantees road tiles are
   *                  walkable; this is belt-and-suspenders, not a behaviour change).
   */
  constructor(roads: readonly Road[], isBlocked?: TileBlocked) {
    for (const road of roads) {
      const path = road.path;
      // Only roads with a real routed polyline (>= 2 waypoints) are walkable.
      // Skip self-loops and missing endpoints — they can't form a usable edge.
      if (!path || path.length < 2) continue;
      if (!road.from || !road.to || road.from === road.to) continue;
      // DEFENSIVE: skip an edge whose polyline would put a walker on blocked water
      // (open sea / un-bridged river). The backend guarantees this never happens,
      // so in practice no edge is dropped — but a degenerate/buggy polyline can
      // never leak a citizen onto the sea.
      if (isBlocked && pathTouchesBlocked(path, isBlocked)) continue;

      // Store the polyline in the road's declared from→to orientation under the
      // `from` node, and its reverse under the `to` node, so each direction of
      // traversal reads its waypoints in walking order with no per-route reversal
      // cost beyond the one done here, once.
      const forward = path.map((p) => ({ x: p.x, y: p.y }));
      const backward = forward.slice().reverse();
      const weight = Math.max(1, road.weight || 1);
      this.addEdge(road.from, { to: road.to, path: forward, weight });
      this.addEdge(road.to, { to: road.from, path: backward, weight });
    }

    // Sort each node's neighbours by fileId for a stable, deterministic BFS
    // expansion order (lexicographic). Done once at build time.
    for (const edges of this.adj.values()) {
      edges.sort((a, b) => (a.to < b.to ? -1 : a.to > b.to ? 1 : 0));
    }
  }

  private addEdge(from: string, edge: Edge): void {
    let list = this.adj.get(from);
    if (!list) {
      list = [];
      this.adj.set(from, list);
    }
    list.push(edge);
  }

  /** True if a building has at least one incident routed road (i.e. is a node
   *  in the graph). Buildings with no roads can never be walked to/from. */
  has(fileId: string): boolean {
    return this.adj.has(fileId);
  }

  /** Number of nodes (buildings with at least one routed road). */
  get nodeCount(): number {
    return this.adj.size;
  }

  /** All node fileIds (buildings with at least one routed road). Used by the
   *  decorative ambient crowd to pick wander targets. */
  get nodeIds(): string[] {
    // Cached: immutable graph → stable result. Read-only to callers.
    if (this.cachedNodeIds === null) this.cachedNodeIds = [...this.adj.keys()];
    return this.cachedNodeIds;
  }

  /**
   * Per-node "busy-ness" weight: the SUM of the import weights of every road
   * incident to each node, returned aligned with {@link nodeIds} (same order).
   * A node fed by many/heavy roads scores high; a leaf node low. Used ONLY by
   * the DECORATIVE ambient crowd to bias foot traffic toward arterials so the
   * busy avenues read as lively — it carries NO real-data meaning and never
   * touches navigation/BFS. Allocated once per graph build (cheap), not per pick.
   */
  get nodeWeights(): number[] {
    // Cached: immutable graph → stable result. Read-only to callers.
    if (this.cachedNodeWeights !== null) return this.cachedNodeWeights;
    const out: number[] = [];
    for (const edges of this.adj.values()) {
      let sum = 0;
      for (const e of edges) sum += e.weight;
      // Guard: a node always has >=1 incident edge here, but floor at 1 so a
      // degenerate zero-weight node still has a nonzero pick probability.
      out.push(sum > 0 ? sum : 1);
    }
    this.cachedNodeWeights = out;
    return out;
  }

  /**
   * BFS the building graph for a route `from` → `to` and return the ordered ISO
   * waypoints to walk: the concatenation of each traversed road's cartesian
   * polyline (oriented in the direction of travel), converted via `cartToIso`.
   *
   * Returns null when:
   *   - from === to (no travel needed — caller keeps the in-place pose),
   *   - either endpoint is not a node (a building with no routed roads),
   *   - the two are in different connected components (no honest path),
   *   - the search exceeds {@link ROUTE_VISIT_CAP} expansions.
   *
   * DETERMINISTIC: neighbours are pre-sorted by fileId, so BFS discovers each
   * node along the lexicographically-stable shortest (fewest-roads) path; a
   * given (from, to) always yields the same waypoint list.
   *
   * The returned polyline is de-duplicated at road joints: consecutive roads
   * share the junction waypoint (road A ends where road B begins), so we drop
   * the duplicated first point of each appended road. The caller may further
   * collapse a leading waypoint that coincides with the agent's current pos.
   */
  findRoute(from: string, to: string): IsoPoint[] | null {
    if (from === to) return null;
    if (!this.adj.has(from) || !this.adj.has(to)) return null;

    // BFS, tracking each node's predecessor + the edge taken to reach it, so we
    // can reconstruct the road polylines along the discovered path. Reuse the
    // per-instance scratch buffers (cleared here) instead of allocating fresh
    // each call — same logic, no per-call GC churn.
    const cameFrom = this.scratchCameFrom;
    const visited = this.scratchVisited;
    const queue = this.scratchQueue;
    cameFrom.clear();
    visited.clear();
    queue.length = 0;
    visited.add(from);
    queue.push(from);
    let head = 0;
    let expansions = 0;
    let found = false;

    while (head < queue.length) {
      const node = queue[head++];
      if (++expansions > ROUTE_VISIT_CAP) return null; // bounded
      if (node === to) {
        found = true;
        break;
      }
      const edges = this.adj.get(node);
      if (!edges) continue;
      for (const edge of edges) {
        if (visited.has(edge.to)) continue;
        visited.add(edge.to);
        cameFrom.set(edge.to, { prev: node, edge });
        queue.push(edge.to);
      }
    }

    if (!found) return null;

    // Reconstruct the ordered list of edges (roads) from `to` back to `from`.
    // Reuse the per-instance scratch buffer (cleared here).
    const edges = this.scratchEdges;
    edges.length = 0;
    let cur = to;
    while (cur !== from) {
      const step = cameFrom.get(cur);
      if (!step) return null; // unreachable (shouldn't happen if found)
      edges.push(step.edge);
      cur = step.prev;
    }
    edges.reverse(); // now from → to order

    // Concatenate each road's cartesian polyline (already oriented in travel
    // direction by construction) into one ISO waypoint list, dropping the
    // duplicated junction point shared between consecutive roads.
    const out: IsoPoint[] = [];
    for (let e = 0; e < edges.length; e++) {
      const path = edges[e].path;
      const startIdx = e === 0 ? 0 : 1; // skip shared junction waypoint
      for (let i = startIdx; i < path.length; i++) {
        out.push(cartToIso(path[i].x, path[i].y));
      }
    }

    // A degenerate route with < 2 points carries no real travel — treat as none.
    return out.length >= 2 ? out : null;
  }
}
