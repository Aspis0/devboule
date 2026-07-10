// A6 — UH walk-cycle sprites for the anonymous ambient crowd.
//
// Covers the pure direction-bucket math, the all-or-nothing frame-table
// resolution, and the spawn split: "citizen" walkers carry a Sprite while
// role-typed walkers (and the whole crowd without a bank) stay procedural.

import { describe, expect, it } from "vitest";
import { Container, Texture } from "pixi.js";
import {
  AmbientLayer,
  AMBIENT_TYPES,
  buildWalkFrameTable,
  walkDirBucket,
} from "./AmbientLayer";
import { SpriteBank } from "./spriteAssets";

const DIRS = [0, 45, 90, 135, 180, 225, 270, 315];

function fullWalkBank(omit?: string): SpriteBank {
  const textures = new Map<string, Texture>();
  for (const sex of ["m", "f"]) {
    for (const d of DIRS) {
      for (let f = 0; f < 4; f++) {
        const key = `walk:citizen${sex}:${d}:f${f}`;
        if (key === omit) continue;
        textures.set(key, Texture.WHITE);
      }
    }
  }
  return new SpriteBank(textures, new Map());
}

describe("walkDirBucket", () => {
  it("maps the eight screen octants (y-down) to buckets 0..7", () => {
    expect(walkDirBucket(1, 0)).toBe(0); // east
    expect(walkDirBucket(1, 1)).toBe(1); // south-east
    expect(walkDirBucket(0, 1)).toBe(2); // south (toward camera)
    expect(walkDirBucket(-1, 1)).toBe(3); // south-west
    expect(walkDirBucket(-1, 0)).toBe(4); // west
    expect(walkDirBucket(-1, -1)).toBe(5); // north-west
    expect(walkDirBucket(0, -1)).toBe(6); // north
    expect(walkDirBucket(1, -1)).toBe(7); // north-east
  });

  it("keeps iso road diagonals (2:1 slope) on the pure diagonal buckets", () => {
    // A screen-space leg along an iso tile edge moves 2px x per 1px y — that
    // must land in the adjacent diagonal bucket, never in pure E/W.
    expect(walkDirBucket(2, 1)).toBe(1);
    expect(walkDirBucket(-2, 1)).toBe(3);
    expect(walkDirBucket(-2, -1)).toBe(5);
    expect(walkDirBucket(2, -1)).toBe(7);
  });
});

describe("buildWalkFrameTable", () => {
  it("returns null without a bank", () => {
    expect(buildWalkFrameTable(null)).toBeNull();
    expect(buildWalkFrameTable(undefined)).toBeNull();
  });

  it("resolves 2 sexes x 8 directions x 4 frames from a full bank", () => {
    const table = buildWalkFrameTable(fullWalkBank());
    expect(table).not.toBeNull();
    for (const sex of ["m", "f"] as const) {
      expect(table![sex]).toHaveLength(8);
      for (const frames of table![sex]) expect(frames).toHaveLength(4);
    }
    // Feet registration flows from the manifest (default when unspecified).
    expect(table!.anchor).toEqual([0.5, 1]);
  });

  it("is all-or-nothing: one missing frame voids the whole table", () => {
    expect(buildWalkFrameTable(fullWalkBank("walk:citizenf:270:f3"))).toBeNull();
  });
});

describe("sprite crowd spawn split", () => {
  function spawnCrowd(bank: SpriteBank | null) {
    const layer = new AmbientLayer(new Container(), undefined, undefined, bank);
    const nodes = ["a.ts", "b.ts"];
    layer.setWorld(
      nodes,
      () => ({ x: 0, y: 0 }),
      () => [
        { x: 0, y: 0 },
        { x: 10, y: 5 },
      ],
    );
    layer.setCount(AMBIENT_TYPES.length); // one walker of every ambient type
    return (layer as unknown as { walkers: { type: string; sprite: unknown; base: { destroyed: boolean } }[] })
      .walkers;
  }

  it("gives the anonymous citizen a sprite and keeps every role procedural", () => {
    const walkers = spawnCrowd(fullWalkBank());
    expect(walkers.length).toBeGreaterThan(0);
    for (const w of walkers) {
      if (w.type === "citizen") expect(w.sprite).not.toBeNull();
      else expect(w.sprite).toBeNull();
    }
    expect(walkers.some((w) => w.type === "citizen")).toBe(true);
  });

  it("stays fully procedural without a bank", () => {
    const walkers = spawnCrowd(null);
    expect(walkers.length).toBeGreaterThan(0);
    for (const w of walkers) expect(w.sprite).toBeNull();
  });
});
