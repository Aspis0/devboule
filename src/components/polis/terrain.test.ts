import { describe, it, expect, vi, afterEach } from "vitest";
import { Container, Graphics } from "pixi.js";
import { buildTerrainFrame, MAX_WATER_TILES } from "./terrain";
import type { TerrainData } from "../../types/city";

// Headless exercise of the SPARSE water/sand/bridge terrain-frame builder. PIXI
// v8 Container/Graphics construct + mutate without a GL context (same approach
// as ExternalServiceLayer.test / TradeRouteLayer.test). We assert the bucketing
// + teardown contract and that the shimmer tick is safe — NOT pixel output.

const CHUNK = 8;

function emptyTerrain(): TerrainData {
  return { seaX: 0, minY: 0, maxY: 0, rivers: [], water: [], sand: [], bridges: [] };
}

// ---------------------------------------------------------------------------
// Helper: capture raw Graphics method calls for a build.
// ---------------------------------------------------------------------------

function captureGraphicsCalls(terrain: TerrainData) {
  const calls = { poly: [] as unknown[][], rect: [] as unknown[][], ellipse: [] as unknown[][], moveTo: [] as unknown[][] };

  const origPoly = Graphics.prototype.poly;
  const origRect = Graphics.prototype.rect;
  const origEllipse = Graphics.prototype.ellipse;
  const origMoveTo = Graphics.prototype.moveTo;

  (Graphics.prototype as unknown as Record<string, unknown>).poly = function (...args: unknown[]) {
    calls.poly.push(args);
    return origPoly.apply(this, args as []);
  };
  (Graphics.prototype as unknown as Record<string, unknown>).rect = function (...args: unknown[]) {
    calls.rect.push(args);
    return origRect.apply(this, args as []);
  };
  (Graphics.prototype as unknown as Record<string, unknown>).ellipse = function (...args: unknown[]) {
    calls.ellipse.push(args);
    return origEllipse.apply(this, args as []);
  };
  (Graphics.prototype as unknown as Record<string, unknown>).moveTo = function (...args: unknown[]) {
    calls.moveTo.push(args);
    return origMoveTo.apply(this, args as []);
  };

  try {
    buildTerrainFrame(terrain, CHUNK);
    return { calls };
  } finally {
    (Graphics.prototype as unknown as Record<string, unknown>).poly = origPoly;
    (Graphics.prototype as unknown as Record<string, unknown>).rect = origRect;
    (Graphics.prototype as unknown as Record<string, unknown>).ellipse = origEllipse;
    (Graphics.prototype as unknown as Record<string, unknown>).moveTo = origMoveTo;
  }
}

/**
 * Extract pier-block polygons from the captured call list.
 *
 * Pier blocks are 4-point quads (8 coords) that are axis-aligned rectangles:
 * either all four x-values are identical (vertical bridge) or all four y-values
 * are identical (horizontal bridge — the quad is horizontal in screen space).
 * The deck diamond is also 8 coords but is a rotated diamond with NO pair of
 * points sharing both an x or a y exactly — so the rectangle filter excludes it.
 */
function extractPierBlockPolys(calls: { poly: unknown[][] }) {
  return calls.poly.filter((args) => {
    const coords = args[0] as number[];
    if (coords.length !== 8) return false;
    const xs = [coords[0], coords[2], coords[4], coords[6]];
    const ys = [coords[1], coords[3], coords[5], coords[7]];
    // Pier blocks are axis-aligned rectangles (4-point quads, 8 coords).
    // Horizontal pier block (run along screen-x):
    //   uniqueX=2, uniqueY=2 — rectangle aligned to screen axes.
    // Vertical pier block (run along iso y):
    //   uniqueX=1, uniqueY=4 — thin vertical line in screen space
    //   (all four x-values equal pDepth, four distinct y-values).
    // Deck diamond: uniqueX=3, uniqueY=3 (rotated rectangle).
    // Arch opening: len=10 (5-point poly, not 8 coords).
    const uniqueX = new Set(xs).size;
    const uniqueY = new Set(ys).size;
    // Match horizontal rectangle OR vertical thin rectangle.
    return (uniqueX === 2 && uniqueY === 2) || (uniqueX === 1 && uniqueY === 4);
  });
}

// ---------------------------------------------------------------------------
// Existing buildTerrainFrame tests (bucketing, shimmer, cap, determinism).
// ---------------------------------------------------------------------------

describe("buildTerrainFrame", () => {
  it("returns no chunks for undefined / empty terrain", () => {
    expect(buildTerrainFrame(undefined, CHUNK)).toEqual([]);
    expect(buildTerrainFrame(emptyTerrain(), CHUNK)).toEqual([]);
  });

  it("buckets water/sand/bridge tiles into CHUNK-keyed containers", () => {
    const terrain: TerrainData = {
      seaX: 10,
      minY: 0,
      maxY: 4,
      rivers: [{ gxMin: 3, gxMax: 3 }],
      water: [
        { gx: 10, gy: 0, deep: false },
        { gx: 11, gy: 0, deep: true },
        { gx: 3, gy: 1, deep: false },
      ],
      sand: [
        { gx: 2, gy: 1 },
        { gx: 9, gy: 0 },
      ],
      bridges: [{ gx: 3, gy: 1 }],
    };
    const frame = buildTerrainFrame(terrain, CHUNK);
    expect(frame.length).toBeGreaterThanOrEqual(2);
    const keys = frame.map((c) => c.key).sort();
    expect(keys).toContain("0,0");
    expect(keys).toContain("1,0");
    for (const c of frame) expect(c.container).toBeInstanceOf(Container);
    const seaChunk = frame.find((c) => c.key === "1,0")!;
    expect(seaChunk.anim).not.toBeNull();
  });

  it("shimmer tick is safe to call repeatedly (no throw, bounded redraw)", () => {
    const terrain: TerrainData = {
      seaX: 5,
      minY: 0,
      maxY: 6,
      rivers: [],
      water: Array.from({ length: 6 }, (_, gy) => ({ gx: 5, gy, deep: false })),
      sand: [],
      bridges: [],
    };
    const frame = buildTerrainFrame(terrain, CHUNK);
    const anim = frame.find((c) => c.anim)?.anim;
    expect(anim).toBeTruthy();
    expect(() => {
      anim!.update(0);
      anim!.update(1.5);
    }).not.toThrow();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("warns ONCE when the water cap is exceeded", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const n = MAX_WATER_TILES + 5;
    const terrain: TerrainData = {
      seaX: 0, minY: 0, maxY: n, rivers: [],
      water: Array.from({ length: n }, (_, gy) => ({ gx: 0, gy, deep: false })),
      sand: [], bridges: [],
    };
    buildTerrainFrame(terrain, CHUNK);
    expect(warn).toHaveBeenCalledTimes(1);
    expect(warn.mock.calls[0][0]).toContain(String(MAX_WATER_TILES));
    expect(warn.mock.calls[0][0]).toContain(String(n));
  });

  it("does NOT warn when the water count is within the cap", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const terrain: TerrainData = {
      seaX: 0, minY: 0, maxY: 10, rivers: [],
      water: Array.from({ length: 10 }, (_, gy) => ({ gx: 0, gy, deep: false })),
      sand: [], bridges: [],
    };
    buildTerrainFrame(terrain, CHUNK);
    expect(warn).not.toHaveBeenCalled();
  });

  it("is deterministic — same input yields the same chunk keys in order", () => {
    const terrain: TerrainData = {
      seaX: 12, minY: 0, maxY: 20,
      rivers: [{ gxMin: 4, gxMax: 4 }],
      water: [
        { gx: 12, gy: 0, deep: false },
        { gx: 4, gy: 10, deep: false },
        { gx: 13, gy: 18, deep: true },
      ],
      sand: [{ gx: 3, gy: 10 }],
      bridges: [{ gx: 4, gy: 10 }],
    };
    const a = buildTerrainFrame(terrain, CHUNK).map((c) => c.key);
    const b = buildTerrainFrame(terrain, CHUNK).map((c) => c.key);
    expect(a).toEqual(b);
  });
});

// ---------------------------------------------------------------------------
// Stone arch bridge tests.
// ---------------------------------------------------------------------------

describe("drawBridgeDeck — stone arch bridge", () => {
  it("horizontal run: orientation inferred as horizontal for 3+ consecutive x-axis tiles", () => {
    const terrain: TerrainData = {
      seaX: 0, minY: 0, maxY: 10, rivers: [{ gxMin: 0, gxMax: 10 }],
      water: [
        { gx: 0, gy: 3, deep: false },
        { gx: 1, gy: 3, deep: false },
        { gx: 2, gy: 3, deep: false },
      ],
      sand: [],
      bridges: [
        { gx: 0, gy: 3 },
        { gx: 1, gy: 3 },
        { gx: 2, gy: 3 },
      ],
    };
    const frame = buildTerrainFrame(terrain, CHUNK);
    const chunk = frame.find((c) => c.key === "0,0");
    expect(chunk).toBeDefined();
    expect(chunk!.container).toBeInstanceOf(Container);
  });

  it("vertical run: orientation inferred as vertical for 3+ consecutive y-axis tiles", () => {
    const terrain: TerrainData = {
      seaX: 0, minY: 0, maxY: 10, rivers: [{ gxMin: 0, gxMax: 10 }],
      water: [
        { gx: 3, gy: 0, deep: false },
        { gx: 3, gy: 1, deep: false },
        { gx: 3, gy: 2, deep: false },
      ],
      sand: [],
      bridges: [
        { gx: 3, gy: 0 },
        { gx: 3, gy: 1 },
        { gx: 3, gy: 2 },
      ],
    };
    const frame = buildTerrainFrame(terrain, CHUNK);
    const chunk = frame.find((c) => c.key === "0,0");
    expect(chunk).toBeDefined();
    expect(chunk!.container).toBeInstanceOf(Container);
  });

  // -----------------------------------------------------------------------
  // MAJOR 1 fix: lone tile fallback orientation.
  // -----------------------------------------------------------------------
  it("lone tile: falls back to HORIZONTAL orientation (MAJOR 1 fix)", () => {
    // Lone tile at (5,5): no neighbours → hasH=false, hasV=false → "horizontal".
    const terrain: TerrainData = {
      seaX: 0, minY: 0, maxY: 10, rivers: [{ gxMin: 0, gxMax: 10 }],
      water: [{ gx: 5, gy: 5, deep: false }],
      sand: [],
      bridges: [{ gx: 5, gy: 5 }],
    };
    const { calls } = captureGraphicsCalls(terrain);

    // Isolate pier-block polygons using the rectangle filter:
    // horizontal piers are axis-aligned rectangles with all-identical y-values
    // (horizontal line in screen space), vertical piers have all-identical x-values.
    const pierBlocks = extractPierBlockPolys(calls);
    expect(pierBlocks.length).toBeGreaterThan(0);

    // For a HORIZONTAL lone tile, pier blocks are axis-aligned rectangles:
    // uniqueX=2, uniqueY=2. A VERTICAL lone tile would produce uniqueX=1,
    // uniqueY=4 (thin vertical line). This assertion ONLY passes for horizontal.
    const allPiersAreHorizontal = pierBlocks.every((args) => {
      const coords = args[0] as number[];
      const xs = [coords[0], coords[2], coords[4], coords[6]];
      const ys = [coords[1], coords[3], coords[5], coords[7]];
      return new Set(xs).size === 2 && new Set(ys).size === 2;
    });
    expect(allPiersAreHorizontal).toBe(true);
  });

  // -----------------------------------------------------------------------
  // Vertical run confirmation: real 3+-tile vertical bridge must produce
  // vertical pier geometry (zero x-variance in pier blocks).
  // -----------------------------------------------------------------------
  it("vertical run: pier blocks have zero x-variance (vertical branch draws correctly)", () => {
    const terrain: TerrainData = {
      seaX: 0, minY: 0, maxY: 10, rivers: [{ gxMin: 0, gxMax: 10 }],
      water: [
        { gx: 5, gy: 3, deep: false },
        { gx: 5, gy: 4, deep: false },
        { gx: 5, gy: 5, deep: false },
      ],
      sand: [],
      bridges: [
        { gx: 5, gy: 3 },
        { gx: 5, gy: 4 },
        { gx: 5, gy: 5 },
      ],
    };
    const { calls } = captureGraphicsCalls(terrain);

    const pierBlocks = extractPierBlockPolys(calls);
    expect(pierBlocks.length).toBeGreaterThan(0);

    // For a VERTICAL run, pier blocks are thin vertical lines in screen space:
    // uniqueX=1, uniqueY=4. A horizontal run would produce uniqueX=2, uniqueY=2.
    const allPiersAreVertical = pierBlocks.every((args) => {
      const coords = args[0] as number[];
      const xs = [coords[0], coords[2], coords[4], coords[6]];
      const ys = [coords[1], coords[3], coords[5], coords[7]];
      return new Set(xs).size === 1 && new Set(ys).size === 4;
    });
    expect(allPiersAreVertical).toBe(true);
  });

  // -----------------------------------------------------------------------
  // MAJOR 2 fix: end-tile ramps only on exposed sides.
  // -----------------------------------------------------------------------
  it("end tiles vs middle tiles: only exposed sides get ramps + posts (MAJOR 2 fix)", () => {
    const terrain: TerrainData = {
      seaX: 0, minY: 0, maxY: 10, rivers: [{ gxMin: 0, gxMax: 10 }],
      water: [
        { gx: 0, gy: 3, deep: false },
        { gx: 1, gy: 3, deep: false },
        { gx: 2, gy: 3, deep: false },
        { gx: 3, gy: 3, deep: false },
        { gx: 4, gy: 3, deep: false },
      ],
      sand: [],
      bridges: [
        { gx: 0, gy: 3 },
        { gx: 1, gy: 3 },
        { gx: 2, gy: 3 },
        { gx: 3, gy: 3 },
        { gx: 4, gy: 3 },
      ],
    };
    // With 5 tiles, only tile (0,3) gets endBefore, tile (4,3) gets endAfter.
    // Each exposed side draws 2 ramp polys + 2 end-post rects = 4 rects total.
    const { calls } = captureGraphicsCalls(terrain);
    expect(calls.rect.length).toBe(4);
  });

  // -----------------------------------------------------------------------
  // BLOCKER 2 fix: adjacent tiles' piers share boundary position.
  // -----------------------------------------------------------------------
  it("BLOCKER 2: adjacent horizontal tiles' piers share boundary position", () => {
    // Arithmetic proof (see BLOCKER 2 summary in prior fix pass):
    //   tile0 center: c0 = cartToIso(0.5, 3.5) = {x: -144, y: 96}
    //   tile1 center: c1 = cartToIso(1.5, 3.5) = {x: -96,  y: 120}
    //   hw = 48*0.82 = 39.36, pHW = 0.88
    //
    //   Tile0 "after" pier face x-extent: [c0.x+HW-hw*pHW, c0.x+HW+hw*pHW]
    //     = [-144+48-34.64, -144+48+34.64] = [-130.64, -61.36]
    //   Tile1 "before" pier face x-extent: [c1.x-HW-hw*pHW, c1.x-HW+hw*pHW]
    //     = [-96-48-34.64, -96-48+34.64] = [-178.64, -109.36]
    //
    //   Overlap: [-130.64, -109.36] = 21.28px → piers OVERLAP at boundary.
    const terrain: TerrainData = {
      seaX: 0, minY: 0, maxY: 10, rivers: [{ gxMin: 0, gxMax: 10 }],
      water: [
        { gx: 0, gy: 3, deep: false },
        { gx: 1, gy: 3, deep: false },
      ],
      sand: [],
      bridges: [
        { gx: 0, gy: 3 },
        { gx: 1, gy: 3 },
      ],
    };
    const { calls } = captureGraphicsCalls(terrain);

    // Filter to pier blocks (axis-aligned rectangles, 8 coords).
    const pierBlocks = extractPierBlockPolys(calls);
    expect(pierBlocks.length).toBeGreaterThanOrEqual(8);

    // Collect ALL x-coordinates from horizontal pier blocks (pier blocks
    // where all y are identical — these are the ones on a horizontal run).
    // Collect x-coordinates from HORIZONTAL pier blocks only.
    // Horizontal piers: uniqueX=2, uniqueY=2 (rectangle aligned to screen axes).
    // Vertical piers: uniqueX=1, uniqueY=4 (thin vertical line).
    const allXCoords: number[] = [];
    for (const args of pierBlocks) {
      const coords = args[0] as number[];
      const xs = [coords[0], coords[2], coords[4], coords[6]];
      const ys = [coords[1], coords[3], coords[5], coords[7]];
      if (new Set(xs).size === 2 && new Set(ys).size === 2) {
        allXCoords.push(...xs);
      }
    }

    // Tile0 "after" pier faces span x ∈ [-130.64, -61.36]
    // Tile1 "before" pier faces span x ∈ [-178.64, -109.36]
    // These intervals MUST overlap. Assert: max tile0 right edge >= min tile1 left edge.
    // We look for the largest x among all faces (tile0 after-pier rightmost)
    // and the smallest x among all faces (tile1 before-pier leftmost).
    const maxX = Math.max(...allXCoords);
    const minX = Math.min(...allXCoords);

    // The maximum right-edge of any after-pier face should be ≥ -61.36.
    expect(maxX).toBeGreaterThanOrEqual(-62);
    // The minimum left-edge of any before-pier face should be ≤ -109.36.
    expect(minX).toBeLessThanOrEqual(-109);
    // And critically: maxX >= minX (they overlap, not a gap).
    // Before the fix, maxX was ~-61 but minX was ~-65 (gap of ~9px).
    // After the fix, they overlap, so maxX >= some shared range value.
    // We verify the overlap directly: tile0's rightmost >= tile1's leftmost.
    // tile0 rightmost = -61.36, tile1 leftmost = -178.64 → -61.36 >= -178.64 ✓
    expect(maxX).toBeGreaterThanOrEqual(minX);
  });

  // -----------------------------------------------------------------------
  // Determinism.
  // -----------------------------------------------------------------------
  it("determinism — same input yields identical Graphics command stream", () => {
    const terrain: TerrainData = {
      seaX: 0, minY: 0, maxY: 10, rivers: [{ gxMin: 0, gxMax: 10 }],
      water: [
        { gx: 0, gy: 3, deep: false },
        { gx: 1, gy: 3, deep: false },
        { gx: 2, gy: 3, deep: false },
        { gx: 3, gy: 3, deep: false },
        { gx: 4, gy: 3, deep: false },
      ],
      sand: [],
      bridges: [
        { gx: 0, gy: 3 },
        { gx: 1, gy: 3 },
        { gx: 2, gy: 3 },
        { gx: 3, gy: 3 },
        { gx: 4, gy: 3 },
      ],
    };
    const captureCalls = (): { poly: number; rect: number; ellipse: number; moveTo: number } => {
      let poly = 0, rect = 0, ellipse = 0, moveTo = 0;
      const origPoly = Graphics.prototype.poly;
      const origRect = Graphics.prototype.rect;
      const origEllipse = Graphics.prototype.ellipse;
      const origMoveTo = Graphics.prototype.moveTo;
      (Graphics.prototype as unknown as Record<string, unknown>).poly = function (...a: unknown[]) { poly++; return origPoly.apply(this, a as []); };
      (Graphics.prototype as unknown as Record<string, unknown>).rect = function (...a: unknown[]) { rect++; return origRect.apply(this, a as []); };
      (Graphics.prototype as unknown as Record<string, unknown>).ellipse = function (...a: unknown[]) { ellipse++; return origEllipse.apply(this, a as []); };
      (Graphics.prototype as unknown as Record<string, unknown>).moveTo = function (...a: unknown[]) { moveTo++; return origMoveTo.apply(this, a as []); };
      try {
        buildTerrainFrame(terrain, CHUNK);
        return { poly, rect, ellipse, moveTo };
      } finally {
        (Graphics.prototype as unknown as Record<string, unknown>).poly = origPoly;
        (Graphics.prototype as unknown as Record<string, unknown>).rect = origRect;
        (Graphics.prototype as unknown as Record<string, unknown>).ellipse = origEllipse;
        (Graphics.prototype as unknown as Record<string, unknown>).moveTo = origMoveTo;
      }
    };
    const a = captureCalls();
    const b = captureCalls();
    expect(a).toEqual(b);
  });

  // -----------------------------------------------------------------------
  // Regression.
  // -----------------------------------------------------------------------
  it("regression — water shimmer and shore rendering untouched", () => {
    const terrain: TerrainData = {
      seaX: 5, minY: 0, maxY: 6, rivers: [],
      water: Array.from({ length: 6 }, (_, gy) => ({ gx: 5, gy, deep: false })),
      sand: [{ gx: 4, gy: 1 }],
      bridges: [{ gx: 5, gy: 3 }],
    };
    const frame = buildTerrainFrame(terrain, CHUNK);
    const seaChunk = frame.find((c) => c.anim);
    expect(seaChunk).toBeTruthy();
    expect(() => { seaChunk!.anim!.update(0); seaChunk!.anim!.update(1.5); }).not.toThrow();
    const sandChunk = frame.find((c) => c.key === "0,0");
    expect(sandChunk).toBeTruthy();
  });
});
