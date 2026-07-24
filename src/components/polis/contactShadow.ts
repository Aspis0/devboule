// Soft contact-shadow policy — single source of truth for buildings,
// procedural props and farm/resource ellipses.
//
// Derived from Unknown Horizons tree art in the prop-0 atlas: a soft,
// roughly-centred contact pool under the canopy, no hard cast direction,
// peak opacity in the ~0.28–0.35 band when rendered as a solid ellipse.
// Directional offsets that fight the baked art are forbidden.

/** Shared contact-shadow parameters. */
export const CONTACT_SHADOW = {
  /** Peak fill alpha for soft contact ellipses. */
  alpha: 0.3,
  /**
   * Screen-space offset from the footprint centre (px, unscaled).
   * Tree art is a soft pool with no strong cast direction — keep at zero.
   * Day-phase layer skew (PolisRenderer shadows layer) supplies subtle motion.
   */
  offsetX: 0,
  offsetY: 0,
} as const;
