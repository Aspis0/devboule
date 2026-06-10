// Isometric projection math for the Polis map.
//
// Classic 2:1 isometric tiles (96px wide, 48px tall). Cartesian tile
// coordinates (x, y) — as produced by the backend layout — are projected to
// screen-space "iso" coordinates and back. These are pure functions with no
// PixiJS dependency so they can be unit-tested and reused by the dev harness.

export const TILE_W = 96;
export const TILE_H = 48;

const HALF_W = TILE_W / 2;
const HALF_H = TILE_H / 2;

/** A point in iso screen space. */
export interface IsoPoint {
  x: number;
  y: number;
}

/**
 * Project a cartesian tile coordinate to iso screen space.
 * Returns `{ x, y }` (not `{ sx, sy }`) so the result drops straight into a
 * PixiJS `DisplayObject.position`.
 */
export function cartToIso(x: number, y: number): IsoPoint {
  return {
    x: (x - y) * HALF_W,
    y: (x + y) * HALF_H,
  };
}

/** Inverse of {@link cartToIso}: iso screen space back to cartesian tiles. */
export function isoToCart(sx: number, sy: number): { x: number; y: number } {
  return {
    x: (sx / HALF_W + sy / HALF_H) / 2,
    y: (sy / HALF_H - sx / HALF_W) / 2,
  };
}

/** Euclidean distance between two iso points. */
export function dist(a: IsoPoint, b: IsoPoint): number {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  return Math.sqrt(dx * dx + dy * dy);
}

/** Linear interpolation between two iso points at parameter `t` in [0, 1]. */
export function lerp(a: IsoPoint, b: IsoPoint, t: number): IsoPoint {
  return {
    x: a.x + (b.x - a.x) * t,
    y: a.y + (b.y - a.y) * t,
  };
}

/**
 * Sort key used to draw nearer (lower) buildings last so they overlap farther
 * ones correctly. In iso space depth increases with (x + y).
 */
export function depthKey(x: number, y: number): number {
  return x + y;
}

/** Darken a packed 0xRRGGBB color by `amount` (0..1). */
export function darken(color: number, amount: number): number {
  const r = (color >> 16) & 0xff;
  const g = (color >> 8) & 0xff;
  const b = color & 0xff;
  const f = 1 - amount;
  const nr = Math.max(0, Math.min(255, Math.round(r * f)));
  const ng = Math.max(0, Math.min(255, Math.round(g * f)));
  const nb = Math.max(0, Math.min(255, Math.round(b * f)));
  return (nr << 16) | (ng << 8) | nb;
}

/** Lighten a packed 0xRRGGBB color toward white by `amount` (0..1). */
export function lighten(color: number, amount: number): number {
  const r = (color >> 16) & 0xff;
  const g = (color >> 8) & 0xff;
  const b = color & 0xff;
  const nr = Math.max(0, Math.min(255, Math.round(r + (255 - r) * amount)));
  const ng = Math.max(0, Math.min(255, Math.round(g + (255 - g) * amount)));
  const nb = Math.max(0, Math.min(255, Math.round(b + (255 - b) * amount)));
  return (nr << 16) | (ng << 8) | nb;
}

/**
 * Linearly blend packed 0xRRGGBB colors `a`→`b` by `t` (0..1). The workhorse for
 * deriving new named shades from two PALETTE entries (e.g. nudging the cream
 * grass toward a meadow green) without introducing a fresh hex literal.
 */
export function blend(a: number, b: number, t: number): number {
  const k = Math.max(0, Math.min(1, t));
  const ar = (a >> 16) & 0xff;
  const ag = (a >> 8) & 0xff;
  const ab = a & 0xff;
  const br = (b >> 16) & 0xff;
  const bg = (b >> 8) & 0xff;
  const bb = b & 0xff;
  const nr = Math.round(ar + (br - ar) * k);
  const ng = Math.round(ag + (bg - ag) * k);
  const nb = Math.round(ab + (bb - ab) * k);
  return (nr << 16) | (ng << 8) | nb;
}

/**
 * Push a packed color's chroma away from its own luma by `amount` (0..1+), i.e.
 * increase saturation while preserving perceived brightness. Lets the palette
 * stay in its families but read as ALIVE rather than washed-out. `amount` may
 * exceed 1 for a stronger boost; channels clamp to [0,255].
 */
export function saturate(color: number, amount: number): number {
  const r = (color >> 16) & 0xff;
  const g = (color >> 8) & 0xff;
  const b = color & 0xff;
  // Rec. 601 luma — the gray we pull each channel away from.
  const luma = 0.299 * r + 0.587 * g + 0.114 * b;
  const f = 1 + amount;
  const nr = Math.max(0, Math.min(255, Math.round(luma + (r - luma) * f)));
  const ng = Math.max(0, Math.min(255, Math.round(luma + (g - luma) * f)));
  const nb = Math.max(0, Math.min(255, Math.round(luma + (b - luma) * f)));
  return (nr << 16) | (ng << 8) | nb;
}
