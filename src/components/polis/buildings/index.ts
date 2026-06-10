// Building ADAPTER — plugs the ported "Claude Design" procedural kit
// (../kitcd/*) into the renderer's BuiltBuilding contract.
//
// `buildBuilding(b, profile, scale)` is THE seam. It:
//   1. maps the building's `visualTier` → the kit's level 0..4
//      (kalybe→0, oikia→1, synoikia→2, megaron→3, mnemeion→4),
//   2. calls BUILDERS[b.purpose] ?? BUILDERS.unknown at that level to get the
//      kit's { container, body, anims, foot },
//   3. wraps that into a BuiltBuilding: the kit container becomes `display`
//      (the renderer positions it at the building's iso/front-bottom point —
//      the kit's makeProj already anchors front-bottom, so local origin (0,0)
//      lines up with cartToIso(coords)); a separate ground ellipse `shadow`
//      grounds it; the kit's live `anims` are returned for the step clock.
//
// DETERMINISM: the kit builders use only fixed geometry (no Math.random); all
// static scatter goes through detail.ts's seeded sin-hash, so a re-scan
// reproduces the same city. Animation randomness (flicker/wave/puff phase) is
// time-based and intentionally left alone.
//
// Oracle-introduced / unknown slugs fall back to BUILDERS.unknown — never an
// invented building.

import { Graphics } from "pixi.js";
import type { Building } from "../../../types/city";
import {
  tierRank,
  tierScale,
  providerLivery,
  DERIVED,
  type BuildingProfile,
} from "../palette";
import { darken, lighten } from "../iso";
import { BUILDERS, type Builder } from "../kitcd/buildings";
import { makeProj, MAT, TILE_W, TILE_H, shade } from "../kitcd/iso";
import type { BuiltBuilding } from "./types";

const HALF_W = TILE_W / 2;
const HALF_H = TILE_H / 2;

/** Resolve the kit builder for a purpose slug (unknown fallback). */
function getBuilder(purpose: string): Builder {
  return BUILDERS[purpose] ?? BUILDERS.unknown;
}

/**
 * Faithful contact-shadow port of the source harness (app.js `shadow()`): a
 * soft ellipse under the footprint centre. The renderer places the returned
 * Graphics at the building's iso anchor (front-bottom), so we bake the
 * front-bottom→centre offset into the geometry via the kit's own projection.
 */
function buildShadow(W: number, D: number): Graphics {
  const g = new Graphics();
  const proj = makeProj(W, D);
  const c = proj.p(W / 2, D / 2, 0); // centre, relative to front-bottom origin
  const rx = (W + D) * HALF_W * 0.42;
  const ry = (W + D) * HALF_H * 0.42;
  g.ellipse(c.x, c.y, rx, ry).fill({ color: MAT.shadow, alpha: 0.13 });
  return g;
}

/**
 * TECH LIVERY (F4): build the small procedural provider PENNANT — a short pole
 * with a triangular pennon in the provider accent — planted on the roof apex.
 * Static (drawn ONCE here, never per frame), allocation-free per frame, and a
 * child of the building container so it is torn down with it. Returns `null`
 * when the building has no (known) provider, so plain local files get nothing.
 *
 * `topY` is the silhouette top in local px (the most-negative y of the kit
 * bounds); the pole rises a touch ABOVE that so the pennon clears the roof.
 */
export function buildPennant(provider: string | undefined, topY: number): Graphics | null {
  const accent = providerLivery(provider);
  if (accent === null) return null;

  const g = new Graphics();
  // Plant at local x=0 (the building's iso center column). topY is negative.
  const poleBase = topY + 2; // just inside the roof so it reads as mounted
  const poleH = 13;
  const poleTop = poleBase - poleH;
  const cloth = 9; // pennon length
  const clothH = 6;

  // Pole (thin, dark) — derived from the palette outline tone.
  g.moveTo(0, poleBase)
    .lineTo(0, poleTop)
    .stroke({ width: 1.5, color: DERIVED.pole, alpha: 0.95 });
  // Triangular pennon flying to the right, in the provider accent.
  const yTop = poleTop + 0.5;
  g.poly([0, yTop, cloth, yTop + clothH / 2, 0, yTop + clothH]).fill({
    color: accent,
    alpha: 0.98,
  });
  // A slim darker hoist band + lighter highlight for a crisp city-builder read.
  g.poly([0, yTop, 1.6, yTop, 1.6, yTop + clothH, 0, yTop + clothH]).fill({
    color: darken(accent, 0.28),
    alpha: 0.9,
  });
  g.moveTo(0.6, yTop + 1)
    .lineTo(cloth * 0.62, yTop + clothH / 2)
    .stroke({ width: 0.8, color: lighten(accent, 0.3), alpha: 0.7 });
  // Static: the pennant never animates and is drawn once.
  return g;
}

/**
 * Build a building's geometry for the renderer. Resolves the kit builder from
 * the purpose slug, maps the visual tier to a kit level, draws, and adapts the
 * result into a BuiltBuilding. `_profile` is accepted for API symmetry.
 */
export function buildBuilding(
  b: Building,
  _profile: BuildingProfile,
  _scale: { w: number; depth: number },
): BuiltBuilding {
  const level = tierRank(b.visualTier); // 0..4 — same map the kit levels use
  const built = getBuilder(b.purpose)(level, { outline: false });
  const [W, D] = built.foot;

  // hw / depth for label placement + hit radius. The kit anchors front-bottom
  // at local (0,0) and rises in -y; getLocalBounds gives the silhouette extent.
  const bounds = built.container.getLocalBounds();
  const hw = Math.max(Math.abs(bounds.x), Math.abs(bounds.x + bounds.width), 1);
  const depth = Math.max(-bounds.y, 1); // bounds.y is the top (most negative) px

  // TECH LIVERY pennant — parented into the kit container so it positions with
  // the building and is destroyed with it. `null` for plain local files.
  const pennant = buildPennant(b.provider, bounds.y);
  if (pennant) built.container.addChild(pennant);

  return {
    display: built.container,
    shadow: buildShadow(W, D),
    anims: built.anims,
    pennant,
    hw,
    depth,
    foot: [W, D],
  };
}

// Re-exported so callers (PolisRenderer) can flip the global sun if needed.
export { SUN } from "../kitcd/iso";
export { tierScale };
export type { BuiltBuilding } from "./types";

// Keep a tiny helper for any caller wanting a paved/plain ground tone match.
export { shade as kitShade };
