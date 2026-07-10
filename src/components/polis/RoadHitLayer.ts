// RoadHitLayer — clickable inter-district roads (semantic import edges).
//
// Inter-district roads are import edges whose consumer and supplier buildings
// live in DIFFERENT districts. These represent cross-boundary dependencies —
// the architectural seams of the codebase — and are now clickable to surface
// the same connection card that merchant porters already trigger.
//
// DESIGN:
//   - Hit nodes: one per inter-district road, placed on the tradeRoutes layer
//     (above road cobble, below buildings — same click-discipline as porters).
//     Each Container holds a PolylineHitArea — an array of per-segment quad
//     Polygons that correctly covers elbow/bend regions without false negatives
//     from even-odd ray casting on self-intersecting geometry.
//   - Hover overlay: ONE shared Graphics added to the root AFTER the hit layer
//     (so it renders on top of hit nodes and porters). Redrawn on pointerover /
//     cleared on pointerout. Renders the hovered road's polyline with a light
//     accent stroke (~2.5px, alpha ~0.5). No per-frame work.
//   - Scale: capped at 600 inter-district roads (top by weight, deterministic
//     tiebreak by roadId). Logged once via console.debug when the cap is hit.
//   - Lifecycle: setWorld() tears down old nodes + rebuilds. clear() for full
//     teardown. Destroyed automatically when the parent layer is destroyed.
//   - LOD gating: eventMode toggled by the renderer's zoom handler so that
//     below the zoom where roads are barely visible, hit-testing is disabled.

import { Container, Graphics, Polygon } from "pixi.js";
import { cartToIso, type IsoPoint } from "./iso";
import { PALETTE } from "./palette";
import type { Road, Building } from "../../types/city";

// ── Constants ───────────────────────────────────────────────────────────

/** Half-width of the hit polygon on each side of the polyline (world px). */
const HIT_HALF_WIDTH = 10;

/** Hover overlay stroke width. */
const HOVER_STROKE_WIDTH = 2.5;

/** Hover overlay alpha. */
const HOVER_ALPHA = 0.5;

/** Maximum inter-district roads for which we build hit nodes. */
const MAX_HIT_ROADS = 600;

// ── PolylineHitArea (per-segment quads) ─────────────────────────────────

/**
 * Custom IHitArea for a polyline with a half-width buffer. Holds one
 * quad Polygon per segment so elbow/bend regions are correctly covered
 * (no false negatives from even-odd ray casting on a self-intersecting
 * single polygon). The containing node's hitArea delegates contains()
 * to the quad that the point falls into.
 */
class PolylineHitArea {
  private quads: Polygon[];

  constructor(isoPath: IsoPoint[], halfWidth: number) {
    this.quads = [];
    if (isoPath.length < 2) return;

    for (let i = 0; i < isoPath.length - 1; i++) {
      const a = isoPath[i];
      const b = isoPath[i + 1];
      const dx = b.x - a.x;
      const dy = b.y - a.y;
      const len = Math.sqrt(dx * dx + dy * dy) || 1;
      // Perpendicular unit vector (rotated 90° clockwise).
      const nx = (-dy / len) * halfWidth;
      const ny = (dx / len) * halfWidth;

      // Quad: a+perp, b+perp, b-perp, a-perp (convex, no self-intersection).
      this.quads.push(
        new Polygon([
          a.x + nx, a.y + ny,
          b.x + nx, b.y + ny,
          b.x - nx, b.y - ny,
          a.x - nx, a.y - ny,
        ]),
      );
    }
  }

  /** IHitArea contract — returns true if (x,y) is inside any quad. */
  contains(x: number, y: number): boolean {
    for (const q of this.quads) {
      if (q.contains(x, y)) return true;
    }
    return false;
  }
}

// ── Layer ───────────────────────────────────────────────────────────────

export class RoadHitLayer {
  private root: Container;
  private hitLayer: Container;
  private overlay: Graphics;
  private onSelectConnection?: (from: string, to: string) => void;
  private hitNodes: Container[] = [];

  constructor(
    root: Container,
    onSelectConnection?: (from: string, to: string) => void,
  ) {
    this.root = root;
    this.onSelectConnection = onSelectConnection;

    // Hit nodes sub-container (invisible — only hit areas matter).
    // Added FIRST so the overlay (added after) renders on top.
    this.hitLayer = new Container();
    this.hitLayer.eventMode = "passive"; // allow taps through to children
    this.root.addChild(this.hitLayer);

    // Shared hover overlay: added to root AFTER hitLayer so it renders on top
    // of hit nodes and porters. NOT hit-testable (no eventMode) — pixi skips
    // it during hit-test, so it never blocks taps on hit nodes below.
    this.overlay = new Graphics();
    this.root.addChild(this.overlay);
  }

  /** Number of inter-district hit nodes (for tests / introspection). */
  get count(): number {
    return this.hitNodes.length;
  }

  // ── Public API ──────────────────────────────────────────────────────

  /**
   * Toggle the layer's hit-testability by zoom LOD. Below the threshold, the
   * layer's eventMode is set to "none" so pixi skips all hit-testing on it
   * (no per-child cost). Above, "passive" so children with "static" can be
   * tapped. Also clears any stale hover overlay.
   */
  setLodVisible(visible: boolean): void {
    this.hitLayer.eventMode = visible ? "passive" : "none";
    // Clear stale hover overlay when hiding — if the pointer was over a road
    // when we zoom out past the LOD threshold, the overlay would stay drawn.
    if (!visible) this.overlay.clear();
  }

  /**
   * Rebuild hit nodes for the current city. Tears down old nodes + clears
   * overlay first.
   * @param roads      Full road list from CityState.
   * @param buildings  Building list (for district resolution).
   */
  setWorld(
    roads: readonly Road[],
    buildings: readonly Building[],
  ): void {
    this.clear();

    // Build lookup maps.
    const buildingById = new Map<string, Building>();
    for (const b of buildings) buildingById.set(b.fileId, b);

    // Filter to inter-district roads (both endpoints resolved + different districts).
    const interDistrict: { road: Road; isoPath: IsoPoint[] }[] = [];
    for (const road of roads) {
      const fromBuilding = buildingById.get(road.from);
      const toBuilding = buildingById.get(road.to);
      if (!fromBuilding || !toBuilding) continue; // missing endpoint → skip
      if (fromBuilding.districtId === toBuilding.districtId) continue; // intra-district → skip

      // Convert the road's world-grid polyline to iso-space, deduping
      // consecutive identical points.
      const isoPath = this.resolveIsoPath(road, fromBuilding, toBuilding);
      if (isoPath.length < 2) continue; // degenerate after dedup → skip

      interDistrict.push({ road, isoPath });
    }

    // Cap at MAX_HIT_ROADS, keeping the heaviest roads. Deterministic tiebreak
    // by roadId so the same city always produces the same set of hit nodes.
    let capped = interDistrict;
    if (interDistrict.length > MAX_HIT_ROADS) {
      console.debug(
        `[RoadHitLayer] ${interDistrict.length} inter-district roads exceeds cap ${MAX_HIT_ROADS} — keeping top ${MAX_HIT_ROADS} by weight`,
      );
      capped = interDistrict
        .sort(
          (a, b) =>
            b.road.weight - a.road.weight ||
            a.road.roadId.localeCompare(b.road.roadId),
        )
        .slice(0, MAX_HIT_ROADS);
    }

    // Create hit nodes.
    for (const { road, isoPath } of capped) {
      const node = this.createHitNode(road, isoPath);
      this.hitLayer.addChild(node);
      this.hitNodes.push(node);
    }
  }

  /** Full teardown: destroy all hit nodes + clear overlay. */
  clear(): void {
    for (const node of this.hitNodes) {
      node.removeFromParent();
      node.destroy({ children: true });
    }
    this.hitNodes = [];
    this.overlay.clear();
  }

  // ── Internal ────────────────────────────────────────────────────────

  /**
   * Resolve the road's polyline into iso-space. Prefers the world-grid routed
   * path; falls back to a straight from→to line using building coords.
   * Consecutive identical points are deduped.
   */
  private resolveIsoPath(
    road: Road,
    fromBuilding: Building,
    toBuilding: Building,
  ): IsoPoint[] {
    let raw: IsoPoint[];
    if (road.path && road.path.length >= 2) {
      raw = road.path.map((p) => cartToIso(p.x, p.y));
    } else {
      // Fallback: straight line from building coords.
      const a = cartToIso(fromBuilding.coords.x, fromBuilding.coords.y);
      const b = cartToIso(toBuilding.coords.x, toBuilding.coords.y);
      raw = [a, b];
    }

    // Dedupe consecutive identical points (zero-length segments produce
    // degenerate zero-area quads that add no hit coverage).
    const deduped: IsoPoint[] = [];
    for (const p of raw) {
      const last = deduped[deduped.length - 1];
      if (!last || Math.abs(p.x - last.x) > 0.01 || Math.abs(p.y - last.y) > 0.01) {
        deduped.push(p);
      }
    }
    return deduped;
  }

  /**
   * Create a single hit node for an inter-district road. The node draws NOTHING
   * visible — only a hit-testable PolylineHitArea. Handles pointerover (hover
   * highlight) and pointertap (connection selection).
   */
  private createHitNode(road: Road, isoPath: IsoPoint[]): Container {
    const container = new Container();
    container.eventMode = "static";
    container.cursor = "pointer";
    container.hitArea = new PolylineHitArea(isoPath, HIT_HALF_WIDTH);

    // Hover: draw the road's polyline into the shared overlay.
    container.on("pointerover", () => {
      this.drawHoverOverlay(isoPath);
    });
    container.on("pointerout", () => {
      this.overlay.clear();
    });

    // Click: surface the connection via the EXISTING channel.
    container.on("pointertap", (e) => {
      e.stopPropagation();
      this.onSelectConnection?.(road.from, road.to);
    });

    return container;
  }

  /**
   * Draw the hover highlight overlay: the road's polyline with a light accent
   * stroke. Clears the overlay first (shared Graphics reused).
   */
  private drawHoverOverlay(isoPath: IsoPoint[]): void {
    this.overlay.clear();
    if (isoPath.length < 2) return;

    this.overlay.moveTo(isoPath[0].x, isoPath[0].y);
    for (let i = 1; i < isoPath.length; i++) {
      this.overlay.lineTo(isoPath[i].x, isoPath[i].y);
    }
    this.overlay.stroke({
      color: PALETTE.goldAccent,
      alpha: HOVER_ALPHA,
      width: HOVER_STROKE_WIDTH,
    });
  }
}
