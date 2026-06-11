// BuildingTextureAtlas — lazy per-VARIANT texture cache for Polis buildings.
//
// WHY: every building used to be a full Container tree (a heavy static Graphics
// body + props + a drop-shadow Graphics) kept alive for the whole session. On a
// large city (~879 buildings) that retained ~1GB of JS heap (~1MB/building) and
// idled at 2.4-2.8GB. The fix: a building's STATIC pixels are identical for every
// building that shares the same VISUAL VARIANT, so we render the static body to
// ONE GPU texture per variant and draw each building as a single batched Sprite
// referencing that shared texture. The heavy per-building Graphics is destroyed
// right after it is captured; only the (tiny) animated parts + overlays stay live.
//
// VARIANT KEY — the full static visual identity of a building. The procedural
// kit builders (kitcd/buildings.ts) take ONLY `(level, opt)`: their geometry,
// props, colors and silhouette are a pure function of the PURPOSE slug (which
// builder) and the LEVEL 0..4 (= tierRank(visualTier)). There is NO per-building
// seed, NO district accent, NO status/sin tint in the STATIC body — every such
// thing (selection ring, hover, status/censor/unknown-bug overlays, agent glow,
// the provider pennant, the animated flame/smoke/flag/water) is an OVERLAY the
// renderer attaches on demand, NOT baked here. So the key is exactly
// `${purpose}:${level}` — two buildings with the same (purpose, level) are
// pixel-identical and share ONE Texture object; a different level (or purpose)
// is a different texture.
//
// SHADOW: the contact shadow is also a pure function of the footprint [W, D],
// which is fixed per (purpose, level). So it is baked into a SECOND per-variant
// texture here and drawn as a shared-texture Sprite on the shadows layer — same
// batching win, no per-building shadow Graphics retained.
//
// LAZY: nothing is rendered until the first request for a variant. The renderer's
// chunked build loop naturally warms the cache (the first building of each variant
// pays one generateTexture; the rest reuse it), so startup is NOT 75 synchronous
// renders up front.

import { Container, Graphics, Texture, Rectangle } from "pixi.js";

/**
 * The minimal renderer surface the atlas needs. PIXI's `Renderer.generateTexture`
 * takes a target display object (+ options) and returns a `Texture`. Declared as a
 * narrow interface so headless tests can inject a fake renderer that returns a
 * stub texture without a real GPU/WebGL context (the vitest env is "node").
 */
export interface TextureSource {
  generateTexture(options: {
    target: Container;
    resolution?: number;
    antialias?: boolean;
    frame?: Rectangle;
  }): Texture;
}

/**
 * One cached variant: the static-body texture, the shadow texture, and the LOCAL
 * bounds frame of the static body at capture time. The frame lets the renderer
 * place a Sprite so the textured pixels land EXACTLY where the old container's
 * local geometry did: a Sprite at world `iso` with `position += (frame.x,
 * frame.y)` and anchor (0,0) reproduces the container's origin-at-iso layout.
 */
export interface BuildingVariant {
  /** Shared GPU texture of the static building body (+ static props). */
  texture: Texture;
  /** Shared GPU texture of the contact shadow ellipse. */
  shadowTexture: Texture;
  /** Local bounds of the static body at capture (x/y are the most-negative px). */
  frame: { x: number; y: number; width: number; height: number };
  /** Local bounds of the shadow at capture (the shadow is centred under foot). */
  shadowFrame: { x: number; y: number; width: number; height: number };
}

/**
 * Resolution cap for generated textures. We honour the device pixel ratio so the
 * baked art stays crisp on a HiDPI display, but CAP it at 2 so a 3x/4x display
 * (or a misreported ratio) can't blow texture memory back up — the whole point of
 * the atlas is to BOUND GPU/heap cost. At 2x a tall ~340px lighthouse variant is
 * ~680px tall in the texture: legible at full zoom, still tiny vs. a retained
 * Graphics tree. Documented cap: min(devicePixelRatio, 2), floored at 1.
 */
export const ATLAS_RESOLUTION_CAP = 2;

/** Resolve the capped, dpr-aware texture resolution (test-overridable arg). */
export function atlasResolution(dpr: number): number {
  if (!Number.isFinite(dpr) || dpr <= 0) return 1;
  return Math.min(Math.max(dpr, 1), ATLAS_RESOLUTION_CAP);
}

/** Canonical variant key: the FULL static visual identity (purpose × level). */
export function variantKey(purpose: string, level: number): string {
  return `${purpose}:${level}`;
}

export class BuildingTextureAtlas {
  private cache = new Map<string, BuildingVariant>();
  private resolution: number;

  /**
   * @param dpr device pixel ratio (e.g. window.devicePixelRatio). Capped by
   *        {@link atlasResolution}. Tests pass 1 for stable frames.
   */
  constructor(dpr = 1) {
    this.resolution = atlasResolution(dpr);
  }

  /** Number of distinct variant textures currently cached (for tests/metrics). */
  get size(): number {
    return this.cache.size;
  }

  /** True iff the (purpose, level) variant texture has already been generated. */
  has(purpose: string, level: number): boolean {
    return this.cache.has(variantKey(purpose, level));
  }

  /**
   * Lazily resolve (and cache) the texture variant for a (purpose, level). On a
   * cache MISS the caller-provided `build` closure constructs the STATIC body
   * Container + shadow Graphics ONCE off-stage; we generateTexture both, destroy
   * the source display objects, and cache the textures. On a HIT nothing is built
   * — the shared textures are returned. The `build` closure is invoked at most
   * once per variant for the atlas's whole lifetime.
   *
   * The renderer supplies `build` (rather than the atlas calling the kit directly)
   * so the atlas stays free of kit/palette imports and is trivially testable: a
   * test passes a `build` that returns a trivial Graphics + a fake renderer that
   * returns a stub texture, and asserts the lazy/shared/destroy contract.
   */
  get(
    renderer: TextureSource,
    purpose: string,
    level: number,
    build: () => { body: Container; shadow: Graphics },
  ): BuildingVariant {
    const key = variantKey(purpose, level);
    const hit = this.cache.get(key);
    if (hit) return hit;

    const { body, shadow } = build();

    // Capture local bounds BEFORE generateTexture (some fake renderers in tests
    // don't mutate the target; real PIXI reads bounds internally anyway).
    const b = body.getLocalBounds();
    const sb = shadow.getLocalBounds();
    const frame = { x: b.x, y: b.y, width: b.width, height: b.height };
    const shadowFrame = { x: sb.x, y: sb.y, width: sb.width, height: sb.height };

    const texture = renderer.generateTexture({
      target: body,
      resolution: this.resolution,
      antialias: true,
    });
    const shadowTexture = renderer.generateTexture({
      target: shadow,
      resolution: this.resolution,
      antialias: true,
    });

    // The source display objects have served their purpose — their pixels live in
    // the GPU textures now. Destroy them so the heavy static Graphics geometry is
    // NOT retained (the whole point: kill the ~1MB/building JS+GPU body tree).
    body.destroy({ children: true });
    shadow.destroy({ children: true });

    const variant: BuildingVariant = {
      texture,
      shadowTexture,
      frame,
      shadowFrame,
    };
    this.cache.set(key, variant);
    return variant;
  }

  /**
   * Release every cached variant texture. Called from PolisRenderer.destroy (the
   * atlas OWNS the textures — a building node never destroys a shared texture, or
   * it would pull the rug out from under every sibling using the same variant).
   * Idempotent: clears the cache so a re-used atlas instance rebuilds lazily.
   */
  destroy(): void {
    for (const v of this.cache.values()) {
      v.texture.destroy(true);
      v.shadowTexture.destroy(true);
    }
    this.cache.clear();
  }
}
