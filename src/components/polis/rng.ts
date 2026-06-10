// Deterministic pseudo-random number generation for the Polis renderer.
//
// DETERMINISM CONTRACT: every visual VARIATION in the map must be reproducible
// from a REAL identifier — a building's `fileId`, or a terrain tile / prop's
// (tileX, tileY). A re-scan of the same project therefore re-creates the exact
// same city, pixel for pixel. There must be NO `Math.random()` anywhere in the
// render path; all variation flows through the helpers here.
//
// Pipeline: string/coords -> 32-bit seed (FNV-1a / xmur3-style mix) ->
// mulberry32 PRNG -> a tiny `Rng` facade with float/int/bool/pick/range/jitter.

/**
 * xmur3-style string hash producing a well-mixed 32-bit seed. (FNV-1a on its
 * own clusters for short, similar strings like sequential UUIDs; the extra
 * avalanche here keeps neighbouring ids visually decorrelated.)
 */
export function hashString(str: string): number {
  let h = 1779033703 ^ str.length;
  for (let i = 0; i < str.length; i++) {
    h = Math.imul(h ^ str.charCodeAt(i), 3432918353);
    h = (h << 13) | (h >>> 19);
  }
  // Final avalanche.
  h = Math.imul(h ^ (h >>> 16), 2246822507);
  h = Math.imul(h ^ (h >>> 13), 3266489909);
  h ^= h >>> 16;
  return h >>> 0;
}

/**
 * Hash a pair of (possibly fractional / negative) tile coordinates to a 32-bit
 * seed. Coordinates are quantised so tiles a fraction apart still map to the
 * same logical tile — terrain/props are tile-grained decoration.
 */
export function hashCoords(x: number, y: number): number {
  // Quantise to integer tile cells, then mix with two large primes (a classic
  // 2D spatial hash) before a final avalanche.
  const xi = Math.floor(x) | 0;
  const yi = Math.floor(y) | 0;
  let h = Math.imul(xi, 374761393) ^ Math.imul(yi, 668265263);
  h = Math.imul(h ^ (h >>> 13), 1274126177);
  h ^= h >>> 16;
  return h >>> 0;
}

/** mulberry32: fast, well-distributed 32-bit PRNG. Returns floats in [0, 1). */
export function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return function () {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/**
 * A small deterministic RNG facade. Build one per building (`rngFromString`) or
 * per tile (`rngFromCoords`); each call advances the stream, so derive every
 * variant of an entity from the SAME Rng in a fixed order to stay reproducible.
 */
export class Rng {
  private next: () => number;

  constructor(seed: number) {
    this.next = mulberry32(seed);
  }

  /** Float in [0, 1). */
  float(): number {
    return this.next();
  }

  /** Float in [min, max). */
  range(min: number, max: number): number {
    return min + this.next() * (max - min);
  }

  /** Integer in [min, max] inclusive. */
  int(min: number, max: number): number {
    return min + Math.floor(this.next() * (max - min + 1));
  }

  /** True with probability `p` (default 0.5). */
  bool(p = 0.5): boolean {
    return this.next() < p;
  }

  /** Symmetric jitter in [-amt, amt). */
  jitter(amt: number): number {
    return (this.next() * 2 - 1) * amt;
  }

  /** Pick one element of a non-empty array. */
  pick<T>(items: readonly T[]): T {
    return items[Math.floor(this.next() * items.length)];
  }
}

/** Build a deterministic Rng from a string identifier (e.g. a building fileId). */
export function rngFromString(id: string): Rng {
  return new Rng(hashString(id));
}

/** Build a deterministic Rng from tile coordinates (terrain / props). */
export function rngFromCoords(x: number, y: number): Rng {
  return new Rng(hashCoords(x, y));
}

/**
 * A single deterministic [0,1) value for coords — handy for value-noise tinting
 * where allocating a full Rng per tile would be wasteful.
 */
export function valueNoise(x: number, y: number): number {
  const h = hashCoords(x, y);
  // One mulberry32 step is enough for a stable per-tile value.
  let t = Math.imul(h ^ (h >>> 15), 1 | h);
  t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
}
