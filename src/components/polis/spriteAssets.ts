// Polis sprite assets — loader + lookup for the real-art sprite atlases
// (docs/polis-sprite-art-plan-2026-07.md).
//
// CONTRACT: real art is an ENHANCEMENT, never a dependency. Every consumer asks
// the SpriteBank for a semantic key and falls back to the procedural kit when
// the answer is null — per FEATURE, not all-or-nothing, so a partially loaded
// (or partially curated) atlas set still upgrades whatever it covers. A missing
// manifest, a failed fetch, the ?sprites=0 harness toggle, and the pre-A3 empty
// manifest all degrade to exactly today's procedural rendering. Every DROPPED
// entry warns exactly once with its cause — a sprite that silently falls back
// is indistinguishable from "not curated yet" and undebuggable.
//
// OWNERSHIP: textures belong to the PIXI.Assets cache (loaded spritesheets),
// NOT to the bank — destroying the bank must not yank textures from live
// sprites. A3 HARD REQUIREMENT: Assets.unload destroys the sheet's textures,
// so the renderer must never unload while sprites reference them (teardown
// order: destroy sprite nodes first, then unload).
//
// DETERMINISM: variant picks flow through rng.ts hashing of REAL identifiers
// (fileId, tile coords) — same project, same city, pixel for pixel. No
// Math.random anywhere (render-path rule).

import { Matrix as PixiMatrix } from "pixi.js";
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

/**
 * Loader for standalone single-PNG textures (the manifest's `singles` map:
 * seamless REPEATING fills — grass, cobble — which can't live in an atlas
 * because GPU wrap-repeat applies to the whole base texture). Production is
 * {@link defaultTextureLoader}, which also flips the texture to repeat mode.
 */
export type TextureLoader = (url: string) => Promise<Texture>;

/** Default anchor: bottom-center — sprite base sits on its iso ground point. */
export const DEFAULT_SPRITE_ANCHOR: readonly [number, number] = [0.5, 1];

/**
 * True when the harness/app location opts out of real art (?sprites=0).
 * Mount-time helper — parses the query string on every call, don't put it on a
 * per-frame path.
 */
export function spritesDisabled(search: string): boolean {
  return new URLSearchParams(search).get("sprites") === "0";
}

/** `${base}:v${n}` — the variant-family key shape pickVariant draws from. */
const VARIANT_RE = /^(.+):v(\d+)$/;

/** Graphics fill style produced by {@link texFillStyle}. */
export interface TexFillStyle {
  texture: Texture;
  matrix: import("pixi.js").Matrix;
  /**
   * LOAD-BEARING: pixi v8 defaults to "local", which stretches the texture
   * across the shape's BOUNDING BOX — on a large polygon that renders one
   * giant blurry copy instead of a repeating carpet. "global" ties UVs to the
   * Graphics' coordinate space so the matrix controls the repeat size and the
   * pattern stays continuous across every shape sharing the Graphics.
   */
  textureSpace: "global";
  color: number;
  alpha: number;
}

/**
 * Build a repeating-texture Graphics fill from a bank single, or null when
 * the bank misses (caller falls back to its flat-color fill). `scale` sizes
 * the repeat: 0.35 ⇒ a 256px source repeats every ~90 world px (~1 tile).
 * The multiply `tint` can only DARKEN the texture — pick light sources.
 */
export function texFillStyle(
  bank: SpriteBank | null | undefined,
  key: string,
  tint: number,
  alpha: number,
  scale = 0.35,
): TexFillStyle | null {
  const texture = bank?.get(key) ?? null;
  if (!texture) return null;
  return {
    texture,
    matrix: new PixiMatrix().scale(scale, scale),
    textureSpace: "global",
    color: tint,
    alpha,
  };
}

/**
 * Resolved sprite lookup. Only FULLY resolved entries are present: an entry
 * whose atlas failed to load (or doesn't exist), or whose frame name is
 * missing from its sheet, is dropped at load time (with a warning) so `get`
 * is a pure cache hit and a null answer always means "use the procedural kit".
 *
 * Variant families are indexed from the keys ACTUALLY resolved, so a hole in
 * the `v0..vN` numbering (generator bug, partially failed atlas) never makes a
 * loaded texture unreachable and never makes a pick land on a missing key.
 */
export class SpriteBank {
  /** base -> ascending list of present variant indexes (e.g. [0, 2, 5]). */
  private variants = new Map<string, number[]>();

  constructor(
    private textures: Map<string, Texture>,
    private metas: Map<string, SpriteEntryMeta>,
  ) {
    for (const key of textures.keys()) {
      const m = VARIANT_RE.exec(key);
      if (!m) continue;
      const list = this.variants.get(m[1]) ?? [];
      list.push(Number(m[2]));
      this.variants.set(m[1], list);
    }
    for (const list of this.variants.values()) list.sort((a, b) => a - b);
  }

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

  /** Number of RESOLVED variants for a base key (holes don't truncate). */
  variantCount(base: string): number {
    return this.variants.get(base)?.length ?? 0;
  }

  /**
   * Deterministically pick a variant key for a real identifier (a building's
   * fileId, a prop's tile key). Same seed string + same resolved variant set
   * -> same variant, forever. Null when the base has no variants — caller
   * falls back to the kit.
   */
  pickVariant(base: string, seed: string): string | null {
    const list = this.variants.get(base);
    if (!list || list.length === 0) return null;
    return `${base}:v${list[hashString(seed) % list.length]}`;
  }
}

/**
 * Load the sprite atlases named by the manifest and resolve every entry.
 * Returns null when there is nothing to draw from (disabled, empty manifest,
 * or every atlas failed) — callers treat null exactly like an empty bank and
 * stay procedural. Per-atlas failures are non-fatal: the other pages' entries
 * still resolve (partial enhancement beats none).
 *
 * Call once per renderer mount and share the bank; concurrent calls are safe
 * (PIXI.Assets dedupes same-url loads and banks share the cached Textures)
 * but produce distinct bank objects — there is deliberately no module-level
 * singleton, the renderer owns its bank like it owns its BuildingTextureAtlas.
 */
export async function loadPolisSprites(opts: {
  loader: AtlasLoader;
  /** Required when the manifest has a `singles` map; unused otherwise. */
  textureLoader?: TextureLoader;
  manifest?: SpriteManifest;
  disabled?: boolean;
}): Promise<SpriteBank | null> {
  const manifest = opts.manifest ?? SPRITE_MANIFEST;
  if (opts.disabled) return null;
  const atlasIds = Object.keys(manifest.atlases);
  const singleKeys = Object.keys(manifest.singles ?? {});
  if (atlasIds.length === 0 && singleKeys.length === 0) return null;

  const pages = new Map<string, Record<string, Texture>>();
  const textures = new Map<string, Texture>();
  await Promise.all([
    ...atlasIds.map(async (id) => {
      try {
        pages.set(id, await opts.loader(manifest.atlases[id]));
      } catch (err) {
        console.warn(`[polis] sprite atlas '${id}' failed to load — its entries stay procedural`, err);
      }
    }),
    ...singleKeys.map(async (key) => {
      const url = manifest.singles![key];
      if (!opts.textureLoader) {
        console.warn(`[polis] sprite single '${key}' skipped — no textureLoader provided`);
        return;
      }
      try {
        textures.set(key, await opts.textureLoader(url));
      } catch (err) {
        console.warn(`[polis] sprite single '${key}' failed to load ('${url}') — stays procedural`, err);
      }
    }),
  ]);
  if (pages.size === 0 && textures.size === 0) return null;

  const metas = new Map<string, SpriteEntryMeta>();
  const unknownAtlases = new Set<string>();
  for (const [key, meta] of Object.entries(manifest.entries)) {
    if (!(meta.atlas in manifest.atlases)) {
      // Distinct from a failed load: the manifest references a page that was
      // never in the load set at all (generator/hand-edit bug). Warn once per
      // unknown id, not per entry.
      if (!unknownAtlases.has(meta.atlas)) {
        unknownAtlases.add(meta.atlas);
        console.warn(`[polis] sprite entries reference unknown atlas id '${meta.atlas}' (e.g. '${key}') — skipped`);
      }
      continue;
    }
    const page = pages.get(meta.atlas);
    if (!page) continue; // atlas failed to load — already warned above
    const texture = page[meta.frame];
    if (!texture) {
      console.warn(`[polis] sprite '${key}' missing frame '${meta.frame}' in atlas '${meta.atlas}' — skipped`);
      continue;
    }
    textures.set(key, texture);
    metas.set(key, meta);
  }

  warnVariantHoles(textures);
  return new SpriteBank(textures, metas);
}

/**
 * The A2 generator emits contiguous `v0..vN-1` families; a hole means an asset
 * was dropped somewhere along the pipeline. Holes are HANDLED (the bank picks
 * from the resolved set) but still warned, so pipeline bugs surface in the
 * harness console instead of as quietly thinner variety.
 */
function warnVariantHoles(textures: Map<string, Texture>): void {
  const maxIdx = new Map<string, { max: number; count: number }>();
  for (const key of textures.keys()) {
    const m = VARIANT_RE.exec(key);
    if (!m) continue;
    const cur = maxIdx.get(m[1]) ?? { max: -1, count: 0 };
    cur.max = Math.max(cur.max, Number(m[2]));
    cur.count++;
    maxIdx.set(m[1], cur);
  }
  for (const [base, { max, count }] of maxIdx) {
    if (count !== max + 1) {
      console.warn(`[polis] sprite family '${base}' has holes (${count} variants, max index v${max}) — picks use the resolved set`);
    }
  }
}

/**
 * Validate the value PIXI.Assets returns for a spritesheet url and extract its
 * frame->Texture map. Assets.load's return type depends on the resolver chain
 * AND on what a previous load cached under the same url — a plain-JSON cache
 * hit has no `.textures`. Throwing here (instead of returning undefined) turns
 * that into a per-atlas load failure upstream: warned + procedural fallback,
 * not a TypeError at first frame lookup.
 */
export function sheetTextures(loaded: unknown, url: string): Record<string, Texture> {
  const textures = (loaded as { textures?: unknown } | null | undefined)?.textures;
  if (!textures || typeof textures !== "object") {
    throw new Error(`[polis] '${url}' did not resolve to a spritesheet (no .textures) — was it loaded as raw JSON elsewhere?`);
  }
  return textures as Record<string, Texture>;
}

/** Production loader: PIXI.Assets spritesheet load (lazy pixi import keeps
 * this module cheap for consumers that only need types/helpers). */
export const defaultAtlasLoader: AtlasLoader = async (url) => {
  const { Assets } = await import("pixi.js");
  return sheetTextures(await Assets.load(url), url);
};

/**
 * Release every manifest-named asset from PIXI.Assets' module-level cache.
 * Call ONLY after all sprites/fills referencing them are destroyed (the
 * teardown contract in the module header): Assets caches Texture objects
 * across app instances, and a destroyed WebGL context leaves cache hits with
 * dead GPU backings on the next mount. Fire-and-forget: unload failures cost
 * memory, not correctness.
 */
export function unloadPolisSpriteAssets(manifest: SpriteManifest = SPRITE_MANIFEST): void {
  const urls = [
    ...Object.values(manifest.atlases),
    ...Object.values(manifest.singles ?? {}),
  ];
  if (urls.length === 0) return;
  void import("pixi.js")
    .then(({ Assets }) => Promise.allSettled(urls.map((u) => Assets.unload(u))))
    .catch(() => {
      /* cache cleanup is best-effort */
    });
}

/**
 * Production single-PNG loader. Flips the texture source to wrap-repeat —
 * singles exist exclusively to be tiled/filled, and the pipeline guarantees
 * pow2 dimensions (repeat requires them on WebGL1-class fallbacks).
 */
export const defaultTextureLoader: TextureLoader = async (url) => {
  const { Assets } = await import("pixi.js");
  const texture = (await Assets.load(url)) as Texture;
  if (!texture?.source) {
    throw new Error(`[polis] '${url}' did not resolve to a Texture`);
  }
  texture.source.addressMode = "repeat";
  // Repeated fills are heavily MINIFIED at city zoom (0.15–0.5): without
  // mipmaps linear sampling averages only 4 texels and the ground degrades
  // into aliased mush. Pow2 sources (pipeline-guaranteed) mipmap cleanly.
  texture.source.autoGenerateMipmaps = true;
  return texture;
};
