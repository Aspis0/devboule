// TradeRouteLayer.ts — DATA-BOUND merchant porters that walk the REAL import
// roads so the busiest dependencies read as the busiest streets.
//
// ┌─────────────────────────────────────────────────────────────────────────┐
// │ DATA BOUNDARY — these porters are NOT scenery and NOT real agents.        │
// │                                                                          │
// │ A porter is a kit `merchant` figure (it draws a goods sack) carrying a    │
// │ delivery along ONE real `Road`. It exists ONLY for a real import edge —   │
// │ it is NEVER part of `city.agents`, carries NO gold arrow / glow (that is  │
// │ the real-agent marker in AgentLayer), and is the merchant figure so it    │
// │ reads as DISTINCT from both the decorative crowd (AmbientLayer, which no  │
// │ longer uses `merchant`) and from real agents. The number of porters on an │
// │ edge is proportional to the road's import `weight`, so a heavy dependency │
// │ literally looks like a busier avenue.                                     │
// └─────────────────────────────────────────────────────────────────────────┘
//
// DIRECTION (decided): in the road model `Road{from,to}`, `from` is the IMPORTER
// (consumer) and `to` is the IMPORTED dependency (supplier) — exactly as the
// InspectSidebar reads it. The backend stores the routed polyline `Road.path`
// oriented from→to (i.e. CONSUMER → SUPPLIER; path[0] sits at the `from`/consumer
// building, path[last] at the `to`/supplier building — same orientation the
// RoadGraph stores under the `from` node). A porter carries goods SUPPLIER →
// CONSUMER, so it traverses the polyline in REVERSE (path[last] → path[0]) and
// LOOPS back to the supplier end to start a fresh delivery — a steady flow.
//
// THRESHOLD: porters spawn ONLY on the TOP-WEIGHT edges, never on the long tail
// of weight-1 imports — an edge qualifies when its `weight >= TRADE_WEIGHT_MIN`
// OR it is among the TRADE_TOP_N busiest edges in the graph. Porter COUNT per
// edge scales with weight (porterCountForWeight), capped per-edge and globally
// (TRADE_PORTERS_GLOBAL_CAP) so a dense graph can't spawn thousands.
//
// PERFORMANCE: merchant figures are pooled (instances are reused across rebuilds
// — never destroyed/recreated on a roads-changed diff, only re-seated), each
// owns a seeded Rng (rng.ts — no Math.random) so the flow is reproducible, the
// step clock only redraws the small figure + nudges position (alloc-free per
// frame), and animation is gated to LOD-visible zoom AND porters whose current
// position falls inside the camera's visible bounds (reusing the renderer's
// cull rectangle) — off-screen porters are skipped entirely.
//
// ZOOM-IN ONLY: the whole layer is hidden below TRADE_LOD_ZOOM (~0.45). There is
// NO zoom-out road-flow animation — roads render exactly as they do today when
// zoomed out.

import { Container, Graphics, Rectangle } from "pixi.js";
import { cartToIso, type IsoPoint } from "./iso";
import { Rng, rngFromString } from "./rng";
import { drawCitizen, defaultTunic, shadeColor } from "./kitcd/people";
import { pathTouchesBlocked } from "./navWalkable";
import type { Road } from "../../types/city";

// Match Agent/Ambient layers so a porter reads at the same on-map size.
const FIGURE_SCALE = 0.55;
const OMINO_Y_OFFSET = -4;

// Porters amble like the crowd (slower than the 70px/s real agents) — a calm,
// steady delivery flow, not a march.
const WALK_SPEED = 40; // iso px/s

// Per-call delta cap (ms). Matches PolisRenderer's MAX_ANIM_DT (0.05s) so a long
// background stall can't blow up `remaining` and spin the wrap loop. See update().
const MAX_STEP_MS = 50;

// Discrete bob offsets (px), cycled on the step clock (same cadence as the crowd).
const BOB_OFFSETS = [0, -1, -2, -1] as const;

// Porters read as part of the street, a touch transparent so the real agents
// (full-alpha + gold arrow) always pop above them.
const TRADE_ALPHA = 0.9;

// ZOOM-IN ONLY gate: porters are drawn/animated ONLY at/above this zoom. Below
// it the whole layer is hidden (roads stay as they render today). Sits just
// above the agent/ambient LOD band (0.35) so porters appear once the streets are
// legibly close, per the decided ~0.45 threshold.
export const TRADE_LOD_ZOOM = 0.45;

// THRESHOLD — top-weight edges only. An edge qualifies for porters when EITHER
// its import weight is heavy (>= TRADE_WEIGHT_MIN) OR it is among the top-N
// busiest edges by weight in the whole graph. This deliberately excludes the
// long tail of weight-1 imports so porters mark the ARTERIALS, not every lane.
const TRADE_WEIGHT_MIN = 3;
const TRADE_TOP_N = 24;

// Per-edge porter count, capped. A heavier import puts proportionally more
// porters on its road (the visual "busier street"), but never more than
// TRADE_PORTERS_PER_EDGE_CAP on any single edge.
const TRADE_PORTERS_PER_EDGE_CAP = 4;

// Global hard cap so a pathologically dense graph (many heavy edges) can't spawn
// thousands of pooled figures. Once reached, further qualifying edges get no
// porters (they still render as roads — porters are an overlay, not the road).
const TRADE_PORTERS_GLOBAL_CAP = 80;

/** Porters for a road of import `weight`: ~1 per 2 weight units, clamped to
 *  [1, cap]. A qualifying edge always gets at least one porter; the heaviest
 *  arterials get the cap. Documented mapping, monotonic in weight. */
function porterCountForWeight(weight: number): number {
  const n = 1 + Math.floor(Math.max(0, weight - 1) / 2);
  return Math.max(1, Math.min(TRADE_PORTERS_PER_EDGE_CAP, n));
}

/** A real import edge a porter walks. `path` is the routed ISO polyline in
 *  SUPPLIER → CONSUMER order (already reversed from the from→to storage), so a
 *  porter just walks it forward and loops. `from`/`to` are the REAL building
 *  fileIds (from = consumer/importer, to = supplier/imported) for click→inspect. */
interface TradeEdge {
  roadId: string;
  /** Consumer (importer) building fileId — the road's `from`. */
  from: string;
  /** Supplier (imported) building fileId — the road's `to`. */
  to: string;
  weight: number;
  /** Routed ISO waypoints, SUPPLIER → CONSUMER (>= 2 points). */
  path: IsoPoint[];
}

interface Porter {
  /** Per-porter deterministic RNG (seeded from roadId + index). */
  rng: Rng;
  /** The edge this porter delivers along (carries the real from/to fileIds). */
  edge: TradeEdge;
  /** Container at `pos` (bobbed vertically, flipped by facing). Interactive. */
  container: Container;
  /** The merchant figure Graphics, cleared + redrawn each visible step. */
  base: Graphics;
  tunic: number;
  /** Current ISO anchor (feet) of the porter. */
  pos: IsoPoint;
  /** Current segment index into `edge.path` (walking path[seg] → path[seg+1]). */
  seg: number;
  /** Param in [0,1) along the current segment. */
  t: number;
  /** Walk-cycle phase (radians) — advances while moving (legs/arms swing). */
  phase: number;
  /** Per-porter integer offset for the stepped bob cadence (0..3). */
  bobPhase: number;
  /** Horizontal facing (+1 right, -1 left). */
  facing: number;
}

/** A click on a porter (or its road) surfaces the REAL import relationship. The
 *  renderer turns these fileIds into Buildings for the InspectSidebar. */
export type TradeConnectionSelect = (from: string, to: string) => void;

export class TradeRouteLayer {
  private root: Container;
  private porters: Porter[] = [];
  // Whether porters are currently allowed to show (set by the zoom LOD gate).
  // Distinct from per-porter visible-chunk culling done each step.
  private lodVisible = false;
  private onSelectConnection?: TradeConnectionSelect;

  constructor(root: Container, onSelectConnection?: TradeConnectionSelect) {
    this.root = root;
    this.onSelectConnection = onSelectConnection;
  }

  /** Number of porters currently pooled (for tests / introspection). */
  get count(): number {
    return this.porters.length;
  }

  /**
   * (Re)build the porter set for the current roads. Called ONLY when the road
   * set actually changed (the renderer gates this behind its roads-changed
   * signature — never on a pure sin/status diff). Picks the qualifying
   * top-weight edges (with a routed polyline + both endpoints resolvable),
   * spawns weight-proportional porters per edge up to the global cap, and tears
   * down the previous pool cleanly first.
   *
   * @param roads   the city's roads (only those with a routed `path` of >= 2
   *                waypoints and distinct, resolvable endpoints form edges).
   * @param resolve maps a building fileId to its iso anchor, or null if not
   *                on-map. Used only to confirm both endpoints exist; the walk
   *                geometry comes from the routed `path` (cartesian → iso).
   */
  setWorld(
    roads: readonly Road[],
    resolve: (fileId: string) => IsoPoint | null,
    isBlocked?: (gx: number, gy: number) => boolean,
  ): void {
    this.clear();

    // 1) Collect candidate edges: a real routed polyline (>= 2 points), distinct
    //    endpoints, and both buildings actually on the map. We DON'T invent a
    //    path — an edge with no routed `path` carries no honest street to walk.
    const candidates: TradeEdge[] = [];
    for (const road of roads) {
      const path = road.path;
      if (!path || path.length < 2) continue;
      if (!road.from || !road.to || road.from === road.to) continue;
      if (!resolve(road.from) || !resolve(road.to)) continue;
      // DEFENSIVE walkability guard: reject an edge whose routed polyline passes
      // through a blocked tile (open sea / un-bridged river) so a porter can never
      // walk onto water. Densifies the run (an interior tile of a horizontal run
      // can cross a 1-wide un-bridged river column even when both corners are dry),
      // mirroring the backend rasterization. The backend guarantees road tiles are
      // walkable, so in practice no edge is dropped; this is belt-and-suspenders.
      if (isBlocked && pathTouchesBlocked(path, isBlocked)) continue;
      const weight = Math.max(1, road.weight || 1);
      // Routed polyline is stored from→to (CONSUMER → SUPPLIER). A porter walks
      // SUPPLIER → CONSUMER, so reverse it once here and convert to iso.
      const iso: IsoPoint[] = [];
      for (let i = path.length - 1; i >= 0; i--) {
        iso.push(cartToIso(path[i].x, path[i].y));
      }
      candidates.push({
        roadId: road.roadId,
        from: road.from,
        to: road.to,
        weight,
        path: iso,
      });
    }
    if (candidates.length === 0) return;

    // 2) THRESHOLD: keep heavy edges (weight >= MIN) plus the top-N by weight, so
    //    the long tail of weight-1 imports never spawns porters. Deterministic
    //    ordering: weight DESC, then roadId ASC (stable across runs).
    const byBusy = [...candidates].sort(
      (a, b) => b.weight - a.weight || (a.roadId < b.roadId ? -1 : a.roadId > b.roadId ? 1 : 0),
    );
    const qualifying = new Set<TradeEdge>();
    for (const e of byBusy) {
      if (e.weight >= TRADE_WEIGHT_MIN) qualifying.add(e);
    }
    for (let i = 0; i < Math.min(TRADE_TOP_N, byBusy.length); i++) {
      qualifying.add(byBusy[i]);
    }

    // 3) Spawn weight-proportional porters per qualifying edge (busiest first so
    //    if the global cap bites, the heaviest arterials keep their porters),
    //    capped per-edge and globally. Pooled merchant figures are created here.
    let spawned = 0;
    for (const edge of byBusy) {
      if (!qualifying.has(edge)) continue;
      if (spawned >= TRADE_PORTERS_GLOBAL_CAP) break;
      const want = porterCountForWeight(edge.weight);
      for (let k = 0; k < want && spawned < TRADE_PORTERS_GLOBAL_CAP; k++) {
        const porter = this.spawn(edge, k, want, spawned);
        this.porters.push(porter);
        spawned++;
      }
    }
  }

  /** Zoom LOD gate (ZOOM-IN ONLY). Above the threshold the layer may show
   *  porters (per-step culling still applies); below it the whole layer is
   *  hidden and no porter is drawn. Called from the renderer's LOD pass. */
  setLodVisible(visible: boolean): void {
    this.lodVisible = visible;
    // Hide the whole sub-tree at once when zoomed out; when zoomed in, leave the
    // containers visible and let the per-step visible-chunk cull decide which
    // porters actually animate/show.
    if (!visible) {
      for (const p of this.porters) p.container.visible = false;
    }
  }

  // -------------------------------------------------------------------------
  // Per-frame (smooth, real ms): advance each porter along its routed polyline,
  // looping back to the supplier end on arrival. Cheap; no allocation.
  // -------------------------------------------------------------------------
  update(deltaMs: number): void {
    if (deltaMs <= 0 || !this.lodVisible || this.porters.length === 0) return;
    // Cap the per-call delta to the same bound the kit-anim/step clock uses
    // (MAX_STEP_MS = 50ms, mirroring PolisRenderer's MAX_ANIM_DT = 0.05s). A long
    // stall (tab backgrounded for seconds) would otherwise make `remaining`
    // enormous; on a tiny-but-nonzero-length route that drains in ~1e-6 px steps,
    // freezing the main thread. Clamping here keeps the leftover-consuming wrap
    // loop bounded per frame.
    const dt = Math.min(deltaMs, MAX_STEP_MS);
    for (const p of this.porters) this.advance(p, dt);
  }

  // -------------------------------------------------------------------------
  // Per-step (retro cadence): bob + figure redraw. LOD- AND visible-chunk-gated.
  // `view` is the renderer's reused visible-world Rectangle (allocation-free).
  // -------------------------------------------------------------------------
  step(frame: number, view: Rectangle): void {
    if (!this.lodVisible) return;
    for (const p of this.porters) {
      // Visible-chunk-only: skip porters whose current position is outside the
      // camera's visible bounds. A small margin covers the figure's own height.
      const onScreen =
        p.pos.x >= view.x - 48 &&
        p.pos.x <= view.x + view.width + 48 &&
        p.pos.y >= view.y - 48 &&
        p.pos.y <= view.y + view.height + 48;
      if (!onScreen) {
        if (p.container.visible) p.container.visible = false;
        continue;
      }
      if (!p.container.visible) p.container.visible = true;

      const bob = BOB_OFFSETS[(Math.floor(frame / 2) + p.bobPhase) % BOB_OFFSETS.length];
      p.container.position.y = p.pos.y + OMINO_Y_OFFSET + bob;

      p.phase += 0.6; // porters are always delivering → legs/arms swing
      drawCitizen(p.base, "merchant", {
        moving: true,
        phase: p.phase,
        actionPhase: 0,
        tunic: p.tunic,
      });
    }
  }

  // -------------------------------------------------------------------------
  // Movement
  // -------------------------------------------------------------------------

  /** Advance a porter along its edge polyline by `deltaMs`. On reaching the
   *  consumer end (path[last]) it LOOPS back to the supplier end (path[0]) to
   *  begin a fresh delivery — a steady flow. Faces the travel direction. */
  private advance(p: Porter, deltaMs: number): void {
    const route = p.edge.path;
    // Total walkable length of the route. A degenerate path (all waypoints
    // coincident, length 0) has nowhere to walk — seat the porter at path[0] and
    // bail so the leftover-consuming wrap below can never spin forever.
    let routeLen = 0;
    for (let i = 0; i < route.length - 1; i++) {
      routeLen += Math.hypot(route[i + 1].x - route[i].x, route[i + 1].y - route[i].y);
    }
    if (routeLen <= 1e-6) {
      const a = route[0];
      const b = route[1] ?? route[0];
      p.seg = 0;
      p.t = 0;
      p.pos = { x: a.x, y: a.y };
      this.faceTowards(p, a, b);
      this.applyTransform(p);
      return;
    }

    let remaining = (WALK_SPEED * deltaMs) / 1000;
    // Walk forward, consuming `remaining` and looping back to the supplier end on
    // arrival WITHOUT discarding the leftover distance (the old reset-and-return
    // dropped it, snapping short routes back to the start every frame). `remaining`
    // shrinks monotonically each segment; on wrap we reset seg/t to 0 and keep
    // walking, so this terminates (it cannot wrap more than `remaining/routeLen`
    // times, and routeLen > 1e-6 here).
    // Belt-and-braces against an unbounded spin: even with routeLen > 1e-6 and a
    // capped deltaMs, a route with one tiny-but-nonzero segment among coincident
    // waypoints could wrap many times. Cap total wraps this frame so the loop can
    // never freeze the main thread.
    let wraps = 0;
    while (remaining > 1e-9) {
      if (p.seg >= route.length - 1) {
        // Reached the consumer end → loop back to the supplier end and carry on
        // with whatever travel is left this frame.
        p.seg = 0;
        p.t = 0;
        if (++wraps > route.length + 8) break;
      }
      const a = route[p.seg];
      const b = route[p.seg + 1];
      const segLen = Math.hypot(b.x - a.x, b.y - a.y) || 1e-6;
      const distLeft = segLen * (1 - p.t);
      if (remaining < distLeft) {
        p.t += remaining / segLen;
        remaining = 0;
      } else {
        remaining -= distLeft;
        p.seg += 1;
        p.t = 0;
      }
    }

    // Clamp onto a real segment (after the loop p.seg may sit at the final index
    // with t=0, i.e. exactly at the consumer end — draw it there).
    const seg = Math.min(p.seg, route.length - 2);
    const a = route[seg];
    const b = route[seg + 1];
    const t = p.seg > seg ? 1 : p.t;
    p.pos = { x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t };
    this.faceTowards(p, a, b);
    this.applyTransform(p);
  }

  private faceTowards(p: Porter, a: IsoPoint, b: IsoPoint): void {
    const dx = b.x - a.x;
    if (dx > 0.01) p.facing = 1;
    else if (dx < -0.01) p.facing = -1;
    p.container.scale.x = p.facing;
  }

  private applyTransform(p: Porter): void {
    p.container.position.set(p.pos.x, p.pos.y + OMINO_Y_OFFSET);
    p.container.scale.x = p.facing;
  }

  // -------------------------------------------------------------------------
  // Construction / teardown
  // -------------------------------------------------------------------------

  /** Spawn porter #`indexOnEdge` (of `countOnEdge` total) on `edge`, staggered
   *  along the polyline so a multi-porter edge reads as a flow spread across the
   *  WHOLE route rather than a clump. `globalIndex` keeps the seed unique across
   *  edges. Deterministic (seeded rng — no Math.random). */
  private spawn(
    edge: TradeEdge,
    indexOnEdge: number,
    countOnEdge: number,
    globalIndex: number,
  ): Porter {
    const rng = rngFromString(`trade:${edge.roadId}:${indexOnEdge}`);
    // Subtle per-porter tunic variation, kept on-palette (same trick as agents).
    const tunic = shadeColor(defaultTunic("merchant"), 0.9 + rng.float() * 0.2);

    const container = new Container();
    container.alpha = TRADE_ALPHA;
    container.visible = false; // shown by the per-step visible-chunk cull
    // Clickable → surface the REAL import relationship. A generous hit area
    // around the small figure; the tap is consumed (stopPropagation) so the
    // viewport background handler doesn't also deselect, and the porter never
    // steals a building's click (porters live on their own sub-container ABOVE
    // roads but the building bodies are on a higher layer and hit-test first).
    container.eventMode = "static";
    container.cursor = "pointer";
    container.hitArea = new Rectangle(-7, -22, 14, 26);
    container.on("pointertap", (e) => {
      e.stopPropagation();
      // from = consumer (importer), to = supplier (imported).
      this.onSelectConnection?.(edge.from, edge.to);
    });

    const base = new Graphics();
    base.scale.set(FIGURE_SCALE);
    container.addChild(base);
    this.root.addChild(container);

    // Stagger the start position along the polyline so porters on the same edge
    // spread out into a flow across the WHOLE route. Even spacing by index over
    // the ACTUAL porter count on this edge (not the constant cap — dividing by
    // the cap would bunch a sub-cap edge's porters into the first part of the
    // route), jittered a little but kept strictly inside the porter's own slot.
    const route = edge.path;
    const lastSeg = Math.max(0, route.length - 1);
    const slots = Math.max(1, countOnEdge);
    const frac = ((indexOnEdge + rng.float() * 0.6) / slots) % 1;
    const seg = Math.min(lastSeg - 1 >= 0 ? lastSeg - 1 : 0, Math.floor(frac * lastSeg));
    const t = frac * lastSeg - seg;
    const a = route[seg];
    const b = route[Math.min(route.length - 1, seg + 1)];
    const startT = Math.max(0, Math.min(0.999, t));
    const pos = { x: a.x + (b.x - a.x) * startT, y: a.y + (b.y - a.y) * startT };

    const phase = rng.float() * Math.PI * 2;
    const bobPhase = globalIndex % BOB_OFFSETS.length;

    const porter: Porter = {
      rng,
      edge,
      container,
      base,
      tunic,
      pos,
      seg,
      t: startT,
      phase,
      bobPhase,
      facing: 1,
    };
    container.position.set(pos.x, pos.y + OMINO_Y_OFFSET);
    this.faceTowards(porter, a, b);
    drawCitizen(base, "merchant", {
      moving: true,
      phase,
      actionPhase: 0,
      tunic,
    });
    return porter;
  }

  /** Tear down every porter cleanly (removeFromParent + destroy) — the L1 leak
   *  pattern the audit caught; do not repeat it. Called on a roads-changed
   *  rebuild and on layer teardown. */
  clear(): void {
    for (const p of this.porters) {
      p.container.removeFromParent();
      p.container.destroy({ children: true });
    }
    this.porters = [];
  }
}
