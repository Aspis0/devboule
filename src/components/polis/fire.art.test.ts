// A7 — real OGA strip frames in the fire atlas bake.
//
// The bake contract is identical with or without art (8 frames per severity,
// atlas owns its textures); what changes is the SOURCE drawn into each
// RenderTexture: a Sprite of the strip frame (art path) vs the procedural
// Graphics ellipses (fallback). The fake renderer records the target's child
// type so both paths are observable without a GPU.

import { describe, expect, it } from "vitest";
import { Texture } from "pixi.js";
import { bakeFireAtlas, type FireSeverity } from "./fire";

const SEVERITIES: FireSeverity[] = ["smoke", "fire", "inferno"];

function makeSpyRenderer() {
  const targets: string[] = [];
  return {
    targets,
    generateTexture: (opts: { target: { children: { constructor: { name: string } }[] } }) => {
      targets.push(opts.target.children[0]?.constructor.name ?? "?");
      return { destroy: () => {} } as unknown as Texture;
    },
  };
}

function bankWith(frames: number): { get(key: string): Texture | null } {
  const keys = new Set(Array.from({ length: frames }, (_, i) => `fx:fire:f${i}`));
  return { get: (key) => (keys.has(key) ? Texture.WHITE : null) };
}

describe("bakeFireAtlas art path", () => {
  it("bakes all flame bands from Sprites when the bank has fx:fire:f0..f7", () => {
    const spy = makeSpyRenderer();
    const atlas = bakeFireAtlas(spy, bankWith(8));
    for (const sev of SEVERITIES) expect(atlas.flames[sev]).toHaveLength(8);
    // 3 severities × 8 frames drawn via Sprite; smoke band stays Graphics.
    expect(spy.targets.filter((t) => t === "Sprite")).toHaveLength(24);
    expect(spy.targets.filter((t) => t === "Graphics")).toHaveLength(6);
  });

  it("falls back to the procedural bands when any frame is missing", () => {
    const spy = makeSpyRenderer();
    const atlas = bakeFireAtlas(spy, bankWith(7)); // f7 missing
    for (const sev of SEVERITIES) expect(atlas.flames[sev]).toHaveLength(8);
    expect(spy.targets.filter((t) => t === "Sprite")).toHaveLength(0);
  });

  it("stays procedural without a bank (historical call shape)", () => {
    const spy = makeSpyRenderer();
    const atlas = bakeFireAtlas(spy);
    for (const sev of SEVERITIES) expect(atlas.flames[sev]).toHaveLength(8);
    expect(spy.targets.filter((t) => t === "Sprite")).toHaveLength(0);
  });
});
