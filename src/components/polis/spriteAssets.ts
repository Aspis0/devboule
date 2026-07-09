// Polis sprite assets — loader + lookup for the real-art sprite atlases
// (docs/polis-sprite-art-plan-2026-07.md).
//
// CONTRACT: real art is an ENHANCEMENT, never a dependency. Every consumer asks
// the SpriteBank for a semantic key and falls back to the procedural kit when
// the answer is null — per FEATURE, not all-or-nothing, so a partially loaded
// (or partially curated) atlas set still upgrades whatever it covers. A missing
// manifest, a failed fetch, the ?sprites=0 harness toggle, and the pre-A3 empty
// manifest all degrade to exactly today's procedural rendering.
//
// OWNERSHIP: textures belong to the PIXI.Assets cache (loaded spritesheets),
// NOT to the bank — destroying the bank must not yank textures from live
// sprites; unloading is a renderer-teardown concern wired when the first
// consumer lands (A3).
//
// DETERMINISM: variant picks flow through rng.ts hashing of REAL identifiers
// (fileId, tile coords) — same project, same city, pixel for pixel. No
// Math.random anywhere (render-path rule).

import type { Texture } from "pixi.js";
import { hashString } from "./rng";
import {
  SPRITE_MANIFEST,
  type SpriteEntryMeta,
  type SpriteManifest,
} from "./spriteManifest";

/**
 * Narrow, injectable atlas loader: spritesheet JSON url -> frame-name ->
 * Texture map. Production uses {@link defaultAtlasLoader} (PIXI.Assets);
 * headless tests inject fakes — same pattern as buildingAtlas's TextureSource.
 */
export type AtlasLoader = (url: string) => Promise<Record<string, Texture>>;

/** Default anchor: bottom-center — sprite base sits on its iso ground point. */
export const DEFAULT_SPRITE_ANCHOR: readonly [number, number] = [0.5, 1];

/** True when the harness/app location opts out of real art (?sprites=0). */
export function spritesDisabled(search: string): boolean {
  return new URLSearchParams(search).get("sprites") === "0";
}

/**
 * Resolved sprite lookup. Only FULLY resolved entries are present: an entry
 * whose atlas failed to load, or whose frame name is missing from its sheet,
 * is dropped at load time (with a warning) so `get` is a pure cache hit and a
 * null answer always means "use the procedural kit".
 */
export class SpriteBank {
  constructor(
    private textures: Map<string, Texture>,
    private metas: Map<string, SpriteEntryMeta>,
  ) {}

  /** Number of resolved sprite entries (for tests/metrics). */
  get size(): number {
    return this.textures.size;
  }

  has(key: string): boolean {
    return this.textures.has(key);
  }

  get(key: string): Texture | null {
    return this.textures.get(key) ?? null;
  }

  meta(key: string): SpriteEntryMeta | null {
    return this.metas.get(key) ?? null;
  }

  /** Anchor for a key, defaulted to bottom-center. */
  anchor(key: string): readonly [number, number] {
    return this.metas.get(key)?.anchor ?? DEFAULT_SPRITE_ANCHOR;
  }

  /**
   * Count of contiguous variants `${base}:v0..vN-1` present in the bank.
   * Contiguity is the generator's invariant; counting stops at the first hole
   * so a partially failed atlas can't make picks land on missing variants.
   */
  variantCount(base: string): number {
    let n = 0;
    while (this.textures.has(`${base}:v${n}`)) n++;
    return n;
  }

  /**
   * Deterministically pick a variant key for a real identifier (a building's
   * fileId, a prop's tile key). Same seed string -> same variant, forever.
   * Null when the base has no variants — caller falls back to the kit.
   */
  pickVariant(base: string, seed: string): string | null {
    const count = this.variantCount(base);
    if (count === 0) return null;
    return `${base}:v${hashString(seed) % count}`;
  }
}

/**
 * Load the sprite atlases named by the manifest and resolve every entry.
 * Returns null when there is nothing to draw from (disabled, empty manifest,
 * or every atlas failed) — callers treat null exactly like an empty bank and
 * stay procedural. Per-atlas failures are non-fatal: the other pages' entries
 * still resolve (partial enhancement beats none).
 */
export async function loadPolisSprites(opts: {
  loader: AtlasLoader;
  manifest?: SpriteManifest;
  disabled?: boolean;
}): Promise<SpriteBank | null> {
  const manifest = opts.manifest ?? SPRITE_MANIFEST;
  if (opts.disabled) return null;
  const atlasIds = Object.keys(manifest.atlases);
  if (atlasIds.length === 0) return null;

  const pages = new Map<string, Record<string, Texture>>();
  await Promise.all(
    atlasIds.map(async (id) => {
      try {
        pages.set(id, await opts.loader(manifest.atlases[id]));
      } catch (err) {
        console.warn(`[polis] sprite atlas '${id}' failed to load — its entries stay procedural`, err);
      }
    }),
  );
  if (pages.size === 0) return null;

  const textures = new Map<string, Texture>();
  const metas = new Map<string, SpriteEntryMeta>();
  for (const [key, meta] of Object.entries(manifest.entries)) {
    const page = pages.get(meta.atlas);
    if (!page) continue; // whole atlas failed — already warned once above
    const texture = page[meta.frame];
    if (!texture) {
      console.warn(`[polis] sprite '${key}' missing frame '${meta.frame}' in atlas '${meta.atlas}' — skipped`);
      continue;
    }
    textures.set(key, texture);
    metas.set(key, meta);
  }
  return new SpriteBank(textures, metas);
}

/** Production loader: PIXI.Assets spritesheet load (lazy pixi import keeps
 * this module cheap for consumers that only need types/helpers). */
export const defaultAtlasLoader: AtlasLoader = async (url) => {
  const { Assets } = await import("pixi.js");
  const sheet = await Assets.load(url);
  return sheet.textures as Record<string, Texture>;
};
