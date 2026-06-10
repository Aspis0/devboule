import { describe, it, expect, vi, afterEach } from "vitest";
import { Container } from "pixi.js";
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
      // Two distinct chunks: tiles near (0,0) and tiles near (16,0).
      water: [
        { gx: 10, gy: 0, deep: false },
        { gx: 11, gy: 0, deep: true },
        { gx: 3, gy: 1, deep: false }, // river tile in chunk 0
      ],
      sand: [
        { gx: 2, gy: 1 },
        { gx: 9, gy: 0 },
      ],
      bridges: [{ gx: 3, gy: 1 }],
    };
    const frame = buildTerrainFrame(terrain, CHUNK);
    // seaX=10 → chunk x=1; river gx=3 → chunk x=0 → two chunks.
    expect(frame.length).toBeGreaterThanOrEqual(2);
    const keys = frame.map((c) => c.key).sort();
    expect(keys).toContain("0,0"); // river + sand
    expect(keys).toContain("1,0"); // sea margin

    // Every chunk has a real Container parent-able into the terrain layer.
    for (const c of frame) expect(c.container).toBeInstanceOf(Container);

    // A chunk that has water exposes a shimmer anim; a sand-only chunk does not
    // necessarily. The sea chunk (1,0) has water → anim present.
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
    // Two ticks at different times must not throw.
    expect(() => {
      anim!.update(0);
      anim!.update(1.5);
    }).not.toThrow();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("warns ONCE (does not silently truncate) when the water cap is exceeded", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    // One column of sea, MAX_WATER_TILES + 5 tiles tall: exceeds the cap.
    const n = MAX_WATER_TILES + 5;
    const terrain: TerrainData = {
      seaX: 0,
      minY: 0,
      maxY: n,
      rivers: [],
      water: Array.from({ length: n }, (_, gy) => ({ gx: 0, gy, deep: false })),
      sand: [],
      bridges: [],
    };
    buildTerrainFrame(terrain, CHUNK);
    expect(warn).toHaveBeenCalledTimes(1);
    // The warning names the cap + the real count (honest, not a silent break).
    expect(warn.mock.calls[0][0]).toContain(String(MAX_WATER_TILES));
    expect(warn.mock.calls[0][0]).toContain(String(n));
  });

  it("does NOT warn when the water count is within the cap", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const terrain: TerrainData = {
      seaX: 0,
      minY: 0,
      maxY: 10,
      rivers: [],
      water: Array.from({ length: 10 }, (_, gy) => ({ gx: 0, gy, deep: false })),
      sand: [],
      bridges: [],
    };
    buildTerrainFrame(terrain, CHUNK);
    expect(warn).not.toHaveBeenCalled();
  });

  it("is deterministic — same input yields the same chunk keys in order", () => {
    const terrain: TerrainData = {
      seaX: 12,
      minY: 0,
      maxY: 20,
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
