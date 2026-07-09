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
    return origPoly.apply(this, args as any);
  };
  (Graphics.prototype as unknown as Record<string, unknown>).rect = function (...args: unknown[]) {
    calls.rect.push(args);
    return origRect.apply(this, args as any);
  };
  (Graphics.prototype as unknown as Record<string, unknown>).ellipse = function (...args: unknown[]) {
    calls.ellipse.push(args);
    return origEllipse.apply(this, args as any);
  };
  (Graphics.prototype as unknown as Record<string, unknown>).moveTo = function (...args: unknown[]) {
    calls.moveTo.push(args);
    return origMoveTo.apply(this, args as any);
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
  // Orientation fallback: a lone tile draws as HORIZONTAL. The new iso
  // geometry is distinguished by the arch ellipse position: horizontal puts
  // it on the D-C edge midpoint (cartToIso(gx+0.5, gy+1)), vertical on the
  // B-C edge midpoint (cartToIso(gx+1, gy+0.5)).
  // -----------------------------------------------------------------------
  it("lone tile: falls back to HORIZONTAL orientation (arch on the D-C edge)", () => {
    const terrain: TerrainData = {
      seaX: 0, minY: 0, maxY: 10, rivers: [{ gxMin: 0, gxMax: 10 }],
      water: [{ gx: 5, gy: 5, deep: false }],
      sand: [],
      bridges: [{ gx: 5, gy: 5 }],
    };
    const { calls } = captureGraphicsCalls(terrain);
    // Arch ellipses have ry = WALL * 0.72 = 6.48 (the shadow ellipse is much
    // taller, ry = HH * 0.7 = 16.8).
    const arches = calls.ellipse.filter((args) => Math.abs((args[3] as number) - 6.48) < 0.05);
    expect(arches.length).toBe(1);
    const isoX = (x: number, y: number) => (x - y) * 48;
    expect(arches[0][0] as number).toBeCloseTo(isoX(5.5, 6), 3); // D-C midpoint (horizontal)
  });

  it("vertical run: arch sits on the B-C edge midpoint per tile", () => {
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
    const arches = calls.ellipse.filter((args) => Math.abs((args[3] as number) - 6.48) < 0.05);
    expect(arches.length).toBe(3);
    const isoX = (x: number, y: number) => (x - y) * 48;
    const got = arches.map((a) => a[0] as number).sort((a, b) => a - b);
    const want = [3, 4, 5].map((gy) => isoX(6, gy + 0.5)).sort((a, b) => a - b);
    for (let i = 0; i < 3; i++) expect(got[i]).toBeCloseTo(want[i], 3);
  });

  // -----------------------------------------------------------------------
  // ISO-BOUNDS invariant (replaces the old pier-offset BLOCKER tests): every
  // vertex the bridge draws must stay inside the span tiles' projected
  // bounding box (padded for lift/wall/parapet/posts). The old screen-axis
  // pier scheme violated this on vertical runs (blocks at +/-HW past the
  // tile); the corner-projected geometry cannot.
  // -----------------------------------------------------------------------
  const assertAllVerticesInBounds = (
    terrain: TerrainData,
    tiles: Array<{ gx: number; gy: number }>,
  ): void => {
    const { calls } = captureGraphicsCalls(terrain);
    const cartToIsoL = (x: number, y: number) => ({ x: (x - y) * 48, y: (x + y) * 24 });
    let minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity;
    for (const t of tiles) {
      for (const [cx, cy] of [[t.gx, t.gy], [t.gx + 1, t.gy], [t.gx + 1, t.gy + 1], [t.gx, t.gy + 1]]) {
        const p = cartToIsoL(cx, cy);
        minX = Math.min(minX, p.x); maxX = Math.max(maxX, p.x);
        minY = Math.min(minY, p.y); maxY = Math.max(maxY, p.y);
      }
    }
    // Pad: LIFT(6) + PARAPET_H(3.5) + POST_H(6) above, WALL(9) + shadow(2) below,
    // shadow ellipse rx = HW*0.7 = 33.6 sideways.
    const padUp = 6 + 3.5 + 6 + 1;
    const padDown = 9 + 2 + 17; // wall + shadow offset + shadow ry
    const padSide = 34;
    for (const args of calls.poly) {
      const coords = args[0] as number[];
      for (let i = 0; i < coords.length; i += 2) {
        expect(coords[i]).toBeGreaterThanOrEqual(minX - padSide);
        expect(coords[i]).toBeLessThanOrEqual(maxX + padSide);
        expect(coords[i + 1]).toBeGreaterThanOrEqual(minY - padUp);
        expect(coords[i + 1]).toBeLessThanOrEqual(maxY + padDown);
      }
    }
  };

  it("vertical bridge: every drawn vertex stays within the span tiles' projected bounds", () => {
    const tiles = [
      { gx: 5, gy: 3 },
      { gx: 5, gy: 4 },
      { gx: 5, gy: 5 },
    ];
    assertAllVerticesInBounds({
      seaX: 0, minY: 0, maxY: 10, rivers: [{ gxMin: 0, gxMax: 10 }],
      water: tiles.map((t) => ({ ...t, deep: false })),
      sand: [],
      bridges: tiles,
    }, tiles);
  });

  it("horizontal bridge: every drawn vertex stays within the span tiles' projected bounds", () => {
    const tiles = [
      { gx: 0, gy: 3 },
      { gx: 1, gy: 3 },
      { gx: 2, gy: 3 },
    ];
    assertAllVerticesInBounds({
      seaX: 0, minY: 0, maxY: 10, rivers: [{ gxMin: 0, gxMax: 10 }],
      water: tiles.map((t) => ({ ...t, deep: false })),
      sand: [],
      bridges: tiles,
    }, tiles);
  });

  // -----------------------------------------------------------------------
  // End posts only on exposed ends: a 5-tile run has 2 exposed edges, each
  // drawing 2 posts x 2 rects (body + lit cap) = 8 rects total; middle
  // tiles draw none.
  // -----------------------------------------------------------------------
  it("end tiles vs middle tiles: only exposed ends get posts", () => {
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
    const { calls } = captureGraphicsCalls(terrain);
    expect(calls.rect.length).toBe(8);
  });

  // -----------------------------------------------------------------------
  // Seamless multi-tile spans: adjacent tiles' deck quads are built from
  // SHARED corner projections (tile0's B/C corners ARE tile1's A/D corners),
  // so the drawn decks join with zero gap.
  // -----------------------------------------------------------------------
  it("adjacent tiles' decks share their boundary corner projections exactly", () => {
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
    const cartToIsoL = (x: number, y: number) => ({ x: (x - y) * 48, y: (x + y) * 24 });
    const LIFT = 6;
    const deckFor = (gx: number, gy: number): number[] => {
      const A = cartToIsoL(gx, gy), B = cartToIsoL(gx + 1, gy),
        C = cartToIsoL(gx + 1, gy + 1), D = cartToIsoL(gx, gy + 1);
      return [A.x, A.y - LIFT, B.x, B.y - LIFT, C.x, C.y - LIFT, D.x, D.y - LIFT];
    };
    const polys = calls.poly.map((args) => args[0] as number[]);
    const findsDeck = (want: number[]): boolean =>
      polys.some((got) => got.length === want.length && got.every((v, i) => Math.abs(v - want[i]) < 0.001));
    expect(findsDeck(deckFor(0, 3))).toBe(true);
    expect(findsDeck(deckFor(1, 3))).toBe(true);
    // The shared edge: tile0's B/C === tile1's A/D by construction.
    const d0 = deckFor(0, 3), d1 = deckFor(1, 3);
    expect(d0.slice(2, 6)).toEqual([d1[0], d1[1], d1[6], d1[7]]);
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
      (Graphics.prototype as unknown as Record<string, unknown>).poly = function (...a: unknown[]) { poly++; return origPoly.apply(this, a as any); };
      (Graphics.prototype as unknown as Record<string, unknown>).rect = function (...a: unknown[]) { rect++; return origRect.apply(this, a as any); };
      (Graphics.prototype as unknown as Record<string, unknown>).ellipse = function (...a: unknown[]) { ellipse++; return origEllipse.apply(this, a as any); };
      (Graphics.prototype as unknown as Record<string, unknown>).moveTo = function (...a: unknown[]) { moveTo++; return origMoveTo.apply(this, a as any); };
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
