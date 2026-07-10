// MAX-RECALL regression tests — props draw-order + rng-stream parity.
//
// (1) Tree Sprites must form one CONTIGUOUS block AFTER every Graphics chunk
//     (interleaving them broke the pixi batch per tree — one draw call each),
//     y-sorted so a south tree paints over a north one.
// (2) Bank presence must not perturb placement: the per-tile rng draws are
//     unconditional, so the same extent places the same prop count with and
//     without a bank (the sprite-load 3s race can never reshape the city).

import { describe, expect, it } from "vitest";
import { Graphics, Sprite, Texture } from "pixi.js";
import { drawProps } from "./props";
import { SpriteBank } from "./spriteAssets";

function treeBank(): SpriteBank {
  const textures = new Map<string, Texture>();
  for (let v = 0; v < 3; v++) {
    textures.set(`prop:tree:v${v}`, Texture.WHITE);
    textures.set(`prop:cypress:v${v}`, Texture.WHITE);
  }
  return new SpriteBank(textures, new Map());
}

const EXT = { minX: 0, maxX: 24, minY: 0, maxY: 24 };

describe("drawProps sprite ordering", () => {
  it("appends tree Sprites as one y-sorted block after all Graphics", () => {
    const { graphics } = drawProps(EXT, new Set(), treeBank());
    const firstSprite = graphics.findIndex((n) => n instanceof Sprite);
    expect(firstSprite).toBeGreaterThan(0); // some trees spawned, after chunks
    for (let i = firstSprite; i < graphics.length; i++) {
      expect(graphics[i]).toBeInstanceOf(Sprite); // contiguous tail block
    }
    const ys = graphics
      .slice(firstSprite)
      .map((s) => (s as Sprite).position.y);
    for (let i = 1; i < ys.length; i++) expect(ys[i]).toBeGreaterThanOrEqual(ys[i - 1]);
  });

  it("places the same prop count with and without a bank (stream parity)", () => {
    const withBank = drawProps(EXT, new Set(), treeBank());
    const withoutBank = drawProps(EXT, new Set(), null);
    expect(withBank.propCount).toBe(withoutBank.propCount);
    expect(withoutBank.graphics.every((n) => n instanceof Graphics)).toBe(true);
  });
});
