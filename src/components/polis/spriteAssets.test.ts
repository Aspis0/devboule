// spriteAssets — loader fallback contract + deterministic variant picking.
//
// The load contract is the safety rail of the whole sprite round: every failure
// shape (disabled, empty manifest, dead atlas, missing frame) must degrade to
// "procedural kit", never throw, never half-resolve an entry.

import { describe, expect, it, vi } from "vitest";
import { Texture } from "pixi.js";
import {
  DEFAULT_SPRITE_ANCHOR,
  loadPolisSprites,
  spritesDisabled,
  type AtlasLoader,
} from "./spriteAssets";
import type { SpriteManifest } from "./spriteManifest";
import { SPRITE_MANIFEST } from "./spriteManifest";
import { hashString } from "./rng";

function tex(): Texture {
  return Texture.EMPTY;
}

function manifest(
  atlases: Record<string, string>,
  entries: SpriteManifest["entries"],
): SpriteManifest {
  return { version: 1, atlases, entries };
}

const TWO_PAGE_MANIFEST = manifest(
  { a: "/polis/atlas/a.json", b: "/polis/atlas/b.json" },
  {
    "prop:olive:v0": { frame: "olive0", atlas: "a" },
    "prop:olive:v1": { frame: "olive1", atlas: "a" },
    "prop:olive:v2": { frame: "olive2", atlas: "a" },
    "tex:grass": { frame: "grass", atlas: "b" },
    "bld:house:2:v0": {
      frame: "house2a",
      atlas: "b",
      foot: [2, 2],
      anchor: [0.5, 0.92],
      hasBakedShadow: true,
    },
  },
);

const PAGES: Record<string, Record<string, Texture>> = {
  "/polis/atlas/a.json": { olive0: tex(), olive1: tex(), olive2: tex() },
  "/polis/atlas/b.json": { grass: tex(), house2a: tex() },
};

const okLoader: AtlasLoader = async (url) => {
  const page = PAGES[url];
  if (!page) throw new Error(`no such atlas: ${url}`);
  return page;
};

describe("loadPolisSprites — fallback contract", () => {
  it("returns null when disabled, regardless of manifest content", async () => {
    const bank = await loadPolisSprites({
      loader: okLoader,
      manifest: TWO_PAGE_MANIFEST,
      disabled: true,
    });
    expect(bank).toBeNull();
  });

  it("returns null on the shipped (empty) manifest — pre-A3 state of record", async () => {
    const loader = vi.fn(okLoader);
    const bank = await loadPolisSprites({ loader, manifest: SPRITE_MANIFEST });
    expect(bank).toBeNull();
    expect(loader).not.toHaveBeenCalled();
  });

  it("resolves every entry when all atlases load", async () => {
    const bank = await loadPolisSprites({
      loader: okLoader,
      manifest: TWO_PAGE_MANIFEST,
    });
    expect(bank).not.toBeNull();
    expect(bank!.size).toBe(5);
    expect(bank!.get("tex:grass")).not.toBeNull();
    expect(bank!.meta("bld:house:2:v0")).toMatchObject({
      foot: [2, 2],
      hasBakedShadow: true,
    });
  });

  it("drops only the dead atlas's entries on partial failure", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    try {
      const loader: AtlasLoader = async (url) => {
        if (url === "/polis/atlas/a.json") throw new Error("404");
        return PAGES[url];
      };
      const bank = await loadPolisSprites({
        loader,
        manifest: TWO_PAGE_MANIFEST,
      });
      expect(bank).not.toBeNull();
      expect(bank!.has("prop:olive:v0")).toBe(false);
      expect(bank!.has("tex:grass")).toBe(true);
      expect(bank!.has("bld:house:2:v0")).toBe(true);
      // One warning for the atlas, none per dropped entry.
      expect(warn).toHaveBeenCalledTimes(1);
    } finally {
      warn.mockRestore();
    }
  });

  it("returns null (not a throw) when every atlas fails", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    try {
      const loader: AtlasLoader = async () => {
        throw new Error("offline");
      };
      const bank = await loadPolisSprites({
        loader,
        manifest: TWO_PAGE_MANIFEST,
      });
      expect(bank).toBeNull();
    } finally {
      warn.mockRestore();
    }
  });

  it("skips an entry whose frame is missing from its (loaded) atlas", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    try {
      const m = manifest(
        { a: "/polis/atlas/a.json" },
        {
          "prop:olive:v0": { frame: "olive0", atlas: "a" },
          "prop:olive:v1": { frame: "NOPE", atlas: "a" },
        },
      );
      const bank = await loadPolisSprites({ loader: okLoader, manifest: m });
      expect(bank!.has("prop:olive:v0")).toBe(true);
      expect(bank!.has("prop:olive:v1")).toBe(false);
      expect(warn).toHaveBeenCalledTimes(1);
    } finally {
      warn.mockRestore();
    }
  });
});

describe("SpriteBank — variants & metadata", () => {
  async function bank() {
    const b = await loadPolisSprites({
      loader: okLoader,
      manifest: TWO_PAGE_MANIFEST,
    });
    return b!;
  }

  it("counts contiguous variants and stops at the first hole", async () => {
    const b = await bank();
    expect(b.variantCount("prop:olive")).toBe(3);
    expect(b.variantCount("tex:grass")).toBe(0); // not a variant family
    expect(b.variantCount("prop:missing")).toBe(0);
  });

  it("holes cap the pick range so picks never land on missing variants", async () => {
    const m = manifest(
      { a: "/polis/atlas/a.json" },
      {
        "prop:olive:v0": { frame: "olive0", atlas: "a" },
        // v1 intentionally absent
        "prop:olive:v2": { frame: "olive2", atlas: "a" },
      },
    );
    const b = (await loadPolisSprites({ loader: okLoader, manifest: m }))!;
    expect(b.variantCount("prop:olive")).toBe(1);
    expect(b.pickVariant("prop:olive", "any-seed")).toBe("prop:olive:v0");
  });

  it("pickVariant is deterministic per seed and spreads across variants", async () => {
    const b = await bank();
    const first = b.pickVariant("prop:olive", "src/foo.ts");
    expect(first).toBe(b.pickVariant("prop:olive", "src/foo.ts"));
    expect(first).toBe(`prop:olive:v${hashString("src/foo.ts") % 3}`);
    const picked = new Set(
      Array.from({ length: 64 }, (_, i) => b.pickVariant("prop:olive", `f${i}`)),
    );
    expect(picked.size).toBeGreaterThan(1); // not everything collapses to one variant
    expect(b.pickVariant("prop:nope", "seed")).toBeNull();
  });

  it("anchor defaults to bottom-center and honors per-entry overrides", async () => {
    const b = await bank();
    expect(b.anchor("tex:grass")).toEqual(DEFAULT_SPRITE_ANCHOR);
    expect(b.anchor("bld:house:2:v0")).toEqual([0.5, 0.92]);
    expect(b.anchor("no:such:key")).toEqual(DEFAULT_SPRITE_ANCHOR);
  });
});

describe("spritesDisabled", () => {
  it("only ?sprites=0 disables", () => {
    expect(spritesDisabled("?sprites=0")).toBe(true);
    expect(spritesDisabled("?foo=1&sprites=0")).toBe(true);
    expect(spritesDisabled("?sprites=1")).toBe(false);
    expect(spritesDisabled("")).toBe(false);
    expect(spritesDisabled("?sprites")).toBe(false);
  });
});
