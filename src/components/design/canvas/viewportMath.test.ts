import { describe, it, expect } from "vitest";
import {
  clampZoom,
  zoomAtPoint,
  wheelZoom,
  worldToScreen,
  screenToWorld,
  nodesBounds,
  fitToBounds,
  MIN_ZOOM,
  MAX_ZOOM,
} from "./viewportMath";
import type { NodeRect } from "../../../types/design";

function rect(over: Partial<NodeRect> = {}): NodeRect {
  return { id: "n", x: 0, y: 0, w: 100, h: 100, z: 1, ...over };
}

describe("clampZoom", () => {
  it("clamps below MIN and above MAX", () => {
    expect(clampZoom(0.1)).toBe(MIN_ZOOM);
    expect(clampZoom(5)).toBe(MAX_ZOOM);
    expect(clampZoom(1)).toBe(1);
  });
  it("clamps NaN to MIN", () => {
    expect(clampZoom(Number.NaN)).toBe(MIN_ZOOM);
  });
});

describe("zoomAtPoint — cursor-anchored zoom keeps the world point under the cursor", () => {
  it("keeps the world coordinate under the cursor fixed on screen", () => {
    const zoom = 1;
    const pan = { x: 50, y: 20 };
    const cx = 200;
    const cy = 120;
    // World point currently under the cursor.
    const before = screenToWorld(cx, cy, pan, zoom);
    const newZoom = 1.6;
    const newPan = zoomAtPoint(zoom, pan, newZoom, cx, cy);
    // That same world point must still project to the cursor at the new zoom.
    const screen = worldToScreen(before.x, before.y, newPan, newZoom);
    expect(screen.x).toBeCloseTo(cx, 6);
    expect(screen.y).toBeCloseTo(cy, 6);
  });
});

describe("wheelZoom", () => {
  it("zooms in on wheel-up (deltaY<0) and clamps", () => {
    const r = wheelZoom(1, { x: 0, y: 0 }, -1, 100, 100);
    expect(r.zoom).toBeGreaterThan(1);
    expect(r.zoom).toBeLessThanOrEqual(MAX_ZOOM);
  });
  it("zooms out on wheel-down and never below MIN", () => {
    let z = 1;
    let pan = { x: 0, y: 0 };
    for (let i = 0; i < 50; i++) {
      const r = wheelZoom(z, pan, 1, 100, 100);
      z = r.zoom;
      pan = r.pan;
    }
    expect(z).toBe(MIN_ZOOM);
  });
});

describe("worldToScreen / screenToWorld round-trip", () => {
  it("is an exact inverse", () => {
    const pan = { x: 33, y: -12 };
    const zoom = 0.75;
    const w = screenToWorld(400, 250, pan, zoom);
    const s = worldToScreen(w.x, w.y, pan, zoom);
    expect(s.x).toBeCloseTo(400, 6);
    expect(s.y).toBeCloseTo(250, 6);
  });
});

describe("nodesBounds", () => {
  it("returns null for an empty set", () => {
    expect(nodesBounds([])).toBeNull();
  });
  it("computes the union bounding box", () => {
    const b = nodesBounds([
      rect({ x: 10, y: 20, w: 100, h: 50 }),
      rect({ x: 200, y: 0, w: 40, h: 300 }),
    ]);
    expect(b).toEqual({ x: 10, y: 0, w: 230, h: 300 });
  });
});

describe("fitToBounds", () => {
  it("returns the default view for null/degenerate bounds (no divide-by-zero)", () => {
    expect(fitToBounds(null, 800, 600)).toEqual({
      zoom: 0.85,
      pan: { x: 40, y: 24 },
    });
    expect(fitToBounds({ x: 0, y: 0, w: 0, h: 0 }, 800, 600).zoom).toBe(0.85);
  });

  it("fits the bounds within the viewport (clamped) and centers them", () => {
    const bounds = { x: 0, y: 0, w: 1000, h: 500 };
    const vw = 800;
    const vh = 600;
    const margin = 80;
    const { zoom, pan } = fitToBounds(bounds, vw, vh, margin);
    // Limiting axis is width: (800-160)/1000 = 0.64.
    expect(zoom).toBeCloseTo(0.64, 6);
    // Scaled box is centered: its screen top-left + half = viewport center.
    const scaledW = bounds.w * zoom;
    const scaledH = bounds.h * zoom;
    const centerX = pan.x + bounds.x * zoom + scaledW / 2;
    const centerY = pan.y + bounds.y * zoom + scaledH / 2;
    expect(centerX).toBeCloseTo(vw / 2, 6);
    expect(centerY).toBeCloseTo(vh / 2, 6);
  });

  it("clamps the fit zoom to MAX for a tiny bounds", () => {
    const { zoom } = fitToBounds({ x: 0, y: 0, w: 10, h: 10 }, 800, 600);
    expect(zoom).toBe(MAX_ZOOM);
  });
});
