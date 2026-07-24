// Shared LOD zoom thresholds for Polis map layers.
//
// Kept in a tiny pure module so unit tests can pin the constants without
// importing the full PixiJS renderer, and so fields vs walls can diverge
// without scattering magic numbers across call sites.

/** Farmland parcels: stay visible a bit further out than fine wall detail. */
export const LOD_FIELDS = 0.22;

/** District boundary walls: keep the phase-2 overview band (walls get noisy
 *  sooner than soft field tints). */
export const LOD_WALLS = 0.3;

/**
 * On-map disaster overlays (legacy Disaster, crowd fires, hero fires, sin-smoke).
 * Disasters matter so they read a touch sooner than fine facade detail — but
 * stay hidden in the far overview so a zoomed-out city is not a field of tiny
 * flames / grey noise.
 */
export const LOD_DISASTER = 0.35;

/** Whether disaster-class effects (fires, sin-smoke) should render at this zoom. */
export function disasterEffectsLodVisible(scale: number): boolean {
  return scale >= LOD_DISASTER;
}
