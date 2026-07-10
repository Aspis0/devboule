// resources.renderer.test.ts — renderer-level resource site sprite tests.
//
// Verifies the rendering contract (in the style of AmbientLayer.sprites.test.ts):
//   1. With a bank containing res keys, N sites produce N interactive sprites
//      (eventMode "static") + rocks in y-sorted order.
//   2. Without the keys, zero site sprites and no crash.

import { describe, expect, it } from "vitest";
import { Container, Sprite, Texture } from "pixi.js";
import { PolisRenderer } from "./PolisRenderer";
import { SpriteBank } from "./spriteAssets";
import type { ResourceSite } from "./resources";

type PixiSprite = InstanceType<typeof Sprite>;
type PixiContainer = InstanceType<typeof Container>;

function resBank(): SpriteBank {
  const textures = new Map<string, Texture>();
  textures.set("res:mine", Texture.WHITE);
  textures.set("res:quarry:v0", Texture.WHITE);
  textures.set("res:quarry:v1", Texture.WHITE);
  for (let v = 0; v < 5; v++) {
    textures.set(`prop:rock:v${v}`, Texture.WHITE);
  }
  return new SpriteBank(textures, new Map());
}

function mkSite(overrides: Partial<ResourceSite> = {}): ResourceSite {
  return {
    id: "res:d1",
    districtId: "d1",
    districtLabel: "Test Quarter",
    kind: "quarry",
    variant: 0,
    gx: 50,
    gy: 50,
    census: { images: 10, fonts: 2, media: 1 },
    ...overrides,
  };
}

function makeFakeRenderer(bank: SpriteBank | null) {
  const resourceSites = new Container() as PixiContainer;
  const fake = Object.create(PolisRenderer.prototype) as InstanceType<typeof PolisRenderer>;
  const set = (k: string, v: unknown) =>
    ((fake as unknown as Record<string, unknown>)[k] = v);

  set("spriteBank", bank);
  set("layers", { resourceSites });
  set("callbacks", {});
  return { fake, resourceSites };
}

// Private-method shim.
const drawResourceSites = (f: unknown, sites: ResourceSite[]) =>
  (f as Record<string, (s: ResourceSite[]) => void>)["drawResourceSites"].call(f, sites);

describe("resource site sprites with bank", () => {
  it("N sites produce N interactive Sprite children in the resourceSites layer", () => {
    const bank = resBank();
    const { fake, resourceSites } = makeFakeRenderer(bank);
    const sites = [
      mkSite({ id: "res:d1", kind: "quarry", variant: 0, gx: 50, gy: 50 }),
      mkSite({ id: "res:d2", kind: "mine", variant: 0, gx: 80, gy: 80 }),
    ];
    drawResourceSites(fake, sites);

    // Each site produces 1 Sprite (the site itself) + 2-4 rocks.
    const allChildren = resourceSites.children;
    expect(allChildren.length).toBeGreaterThan(0);

    // At least N Sprites (the site sprites themselves).
    const sprites = allChildren.filter((c) => c instanceof Sprite);
    expect(sprites.length).toBeGreaterThanOrEqual(sites.length);
  });

  it("children are y-sorted (ascending position.y)", () => {
    const bank = resBank();
    const { fake, resourceSites } = makeFakeRenderer(bank);
    const sites = [
      mkSite({ id: "res:d1", kind: "quarry", gx: 50, gy: 50 }),
      mkSite({ id: "res:d2", kind: "mine", gx: 80, gy: 80 }),
    ];
    drawResourceSites(fake, sites);

    const ys = resourceSites.children.map((c) => (c as PixiSprite).position.y);
    for (let i = 1; i < ys.length; i++) {
      expect(ys[i]).toBeGreaterThanOrEqual(ys[i - 1]);
    }
  });

  it("site sprites are interactive and click fires onSelectResource with the site data", () => {
    const bank = resBank();
    const { fake, resourceSites } = makeFakeRenderer(bank);
    let selectedSite: ResourceSite | null = null;
    (fake as unknown as { callbacks: { onSelectResource: (s: ResourceSite | null) => void } }).callbacks =
      { onSelectResource: (s) => (selectedSite = s) };

    const site = mkSite({ id: "res:d1", kind: "quarry" });
    drawResourceSites(fake, [site]);

    // Find the interactive sprite (eventMode "static" = clickable site sprite).
    const interactive = resourceSites.children.find(
      (c) => (c as PixiSprite).eventMode === "static",
    ) as PixiSprite;
    expect(interactive).toBeDefined();
    expect(interactive.eventMode).toBe("static");
    expect(interactive.cursor).toBe("pointer");

    // Fire the tap — verify the callback receives the correct site.
    interactive.emit("pointertap", { stopPropagation() {} } as unknown as import("pixi.js").FederatedPointerEvent);
    expect(selectedSite).not.toBeNull();
    expect(selectedSite!.id).toBe("res:d1");
    expect(selectedSite!.kind).toBe("quarry");
  });
});

describe("resource site sprites without bank keys", () => {
  it("zero site sprites and no crash when bank lacks res keys", () => {
    const bank = new SpriteBank(new Map(), new Map());
    const { fake, resourceSites } = makeFakeRenderer(bank);
    const sites = [
      mkSite({ id: "res:d1", kind: "quarry" }),
      mkSite({ id: "res:d2", kind: "mine" }),
    ];
    drawResourceSites(fake, sites);

    expect(resourceSites.children.length).toBe(0);
  });

  it("zero site sprites and no crash when bank is null", () => {
    const { fake, resourceSites } = makeFakeRenderer(null);
    const sites = [mkSite({ id: "res:d1", kind: "quarry" })];
    drawResourceSites(fake, sites);

    expect(resourceSites.children.length).toBe(0);
  });
});
