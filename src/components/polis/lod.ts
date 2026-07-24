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
