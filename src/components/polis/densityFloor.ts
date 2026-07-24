// densityFloor.ts — small-city density floor (pure planners).
//
// Tiny repos (e.g. 3-file projects) render as a few buildings in a huge empty
// meadow. When buildings.length < SMALL_CITY_THRESHOLD we:
//   (a) raise prop / forest density factors (more olives + trees near cluster)
//   (b) raise the ambient walker floor (see AmbientLayer MIN_AMBIENT)
//   (c) tighten computeExtent margin so the camera hugs the buildings
//
// HONESTY: no fake buildings. Pure decoration density only.
// DETERMINISM: pure functions of buildingCount (+ optional tier for cap scaling).
// Large cities (≥ threshold) return identity factors / default margin.

/** Buildings below this count trigger the density floor. */
export const SMALL_CITY_THRESHOLD = 30;

/** Default terrain extent margin for large cities (historical). */
export const DEFAULT_EXTENT_MARGIN = 4;

/**
 * Terrain extent margin (tiles) for a city of `buildingCount` buildings.
 * Tiny clusters get a tighter hug so empty meadow doesn't dominate the camera.
 * PURE.
 */
export function extentMarginForBuildingCount(buildingCount: number): number {
  if (buildingCount <= 0) return DEFAULT_EXTENT_MARGIN;
  if (buildingCount < 8) return 2;
  if (buildingCount < SMALL_CITY_THRESHOLD) return 3;
  return DEFAULT_EXTENT_MARGIN;
}

export interface SmallCityDensityFactors {
  /** Multiplier for countryside prop scatter probability / cap. ≥1. */
  propFactor: number;
  /** Multiplier for forest patch count. ≥1. */
  forestFactor: number;
  /** True when the floor is active (buildingCount < threshold). */
  active: boolean;
}

/**
 * Density factors for prop scatter + forest planning.
 * Small city → higher factors; large city → identity (1,1).
 * PURE + deterministic.
 */
export function smallCityDensityFactors(
  buildingCount: number,
): SmallCityDensityFactors {
  if (buildingCount >= SMALL_CITY_THRESHOLD) {
    return { propFactor: 1, forestFactor: 1, active: false };
  }
  const n = Math.max(0, buildingCount);
  // Linear boost: max at 0 buildings, identity at threshold.
  // prop ~1.0..1.85, forest ~1.0..1.7 — tasteful, not a jungle.
  const t = n / SMALL_CITY_THRESHOLD;
  const propFactor = 1 + 0.85 * (1 - t);
  const forestFactor = 1 + 0.7 * (1 - t);
  return { propFactor, forestFactor, active: true };
}
