// fire.ts — P5.1 two-tier building-fire system (flip-book + hero particles).
//
// TIER F1 — CROWD FIRE (always-available fallback, PixiJS v8 Sprite flip-book):
//   - Procedural Flame/Smoke draw code from kitcd/anims.ts is the SOURCE ART.
//   - Baked ONCE per severity band into RenderTexture atlas frames
//     (8 fire frames, 6 smoke frames).
//   - Each burning building is a Sprite whose texture swaps on the StepClock
//     with a per-building seeded phase (hash of fileId — deterministic).
//   - Cost: texture-swap + transform per building per tick. 200 crowd fires ≤ 1ms.
//
// TIER F2 — HERO FIRE (ParticleContainer, PixiJS v8 EXPERIMENTAL):
//   - Pooled ParticleContainers allocated ONCE at init (maxHeroFires from profile).
//   - Re-targeted between buildings on promotion; no per-promotion allocation.
//   - Flames 28–40 + embers 8–12 + smoke 6–10 per fire (≈45–60 particles).
//   - Dynamic props: position/scale/alpha/tint only.
//   - Spawn/decay on StepClock with seeded jitter (hashString, no Math.random).
//   - Severity multiplier: smoke ×1, fire ×1.6, inferno ×2.4.
//   - Flame scale +20% per band.
//   - Promotion: on-screen burning ranked severity desc → distance-to-center asc,
//     capped at maxHeroFires. Re-evaluated on moved/zoomed + sin changes only.
//   - Demotion crossfade 300ms.

import {
  Container,
  Graphics,
  Rectangle,
  Sprite,
  Texture,
  ParticleContainer,
  Particle,
} from "pixi.js";
import { hashString } from "./rng";

// ---- Severity ----

export type FireSeverity = "smoke" | "fire" | "inferno";

const SEVERITY_SPAWN_MULTIPLIER: Record<FireSeverity, number> = {
  smoke: 1.0,
  fire: 1.6,
  inferno: 2.4,
};

const SEVERITY_SCALE: Record<FireSeverity, number> = {
  smoke: 0.7,
  fire: 1.0,
  inferno: 1.2,
};

const SEVERITY_RANK: Record<FireSeverity, number> = {
  inferno: 3,
  fire: 2,
  smoke: 1,
};

/**
 * Whether this severity draws a flame (flip-book or hero particles).
 * Smoke-severity buildings emit soot columns only — never orange fire.
 */
export function crowdFireShowsFlame(severity: FireSeverity): boolean {
  return severity === "fire" || severity === "inferno";
}

/**
 * Sin-smoke look (worst severity === "smoke"): sooty warm-grey column of
 * irregular, overlapping puffs. Must stay visually distinct from ambient
 * activity chimney smoke (cool blue-gray, thin wisp, CHIMNEY_SMOKE_*).
 *
 * Colours sit in a mid warm-grey band — sooty vs cool chimney smoke, but not
 * near-black (a dark mass this size reads as a hole / fog glitch).
 *
 * STEP 3 — silhouette (not architecture): consecutive puffs overlap and merge,
 * radii vary strongly and grow with age, each puff is multi-lobe (bumpy, not a
 * bead), column densest at the roof and dissolving upward, tilted by wind.
 * baseAlpha is the design peak; per-lobe alpha is a fraction of it so stacked
 * overlap does not re-create an opaque dark smear (see lobeAlphaWeight).
 *
 * STEP 1d — ink must fill the band (not a hairline in a padded frame). Geometry
 * is sized so the alpha-nonzero bbox covers ≥60% width / ≥70% height of the
 * bake frame; on-screen size is frame × ink coverage × SMOKE_SPRITE_SCALE.
 */
export const SIN_SMOKE = {
  /** Sooty warm-grey core (mid value — not near-black). */
  colorCore: 0x524a42,
  /** Mid body. */
  colorMid: 0x6b6258,
  /** Soft outer lobe (still warmer/darker than terrain). */
  colorEdge: 0x82786c,
  /**
   * Design peak opacity (user-tuned for limestone + meadow visibility).
   * Per-lobe fills use baseAlpha × lobeAlphaWeight × age-fade — not this raw.
   */
  baseAlpha: 0.8,
  /** Rise (px) over a puff's life. */
  rise: 96,
  /** Horizontal wind lean amplitude over life (column tilt, not per-puff scatter). */
  driftSpan: 34,
  /**
   * Multiplier on baseAlpha for the main body lobe at birth.
   * Overlap composites; 0.42 → single ~0.34, two-puff stack ~0.56, not near-1 smear.
   */
  lobeAlphaWeight: 0.42,
} as const;

/**
 * Shared puff cadence + radius growth for drawSmokeFrame and ink-coverage
 * measurement. lifetime = 1/rate; alive ≈ lifetime/interval ≈ 5–6.
 *
 * STEP 3: vertical spacing = interval × rate × rise must sit *below* the
 * mid-life diameter so consecutive puffs intersect (continuous column), while
 * r0Span keeps neighbours differently sized (not a bead chain of clones).
 */
export const SMOKE_PUFF = {
  interval: 0.34,
  rate: 0.48,
  /** Birth radius range: r0Min .. r0Min+r0Span (wide span → neighbour size pop). */
  r0Min: 7.5,
  r0Span: 6.0,
  /** Radius growth over life: r = (r0 + age * growth) * scale (column widens). */
  growth: 9.5,
  /** Lateral spawn half-width (px at scale 1): x0 ∈ [-pxHalf, +pxHalf]. */
  pxHalf: 3.5,
} as const;

/** Lobes drawn per puff (main body + offset satellites — bumpy silhouette). */
export const SMOKE_LOBES_PER_PUFF = 5;

/** Flame sits this many px above the building iso foot (body, not roof). */
const CROWD_FLAME_FOOT_LIFT = 20;
/** Smoke base sits this many px below the ridge peak (into the roof edge). */
const CROWD_SMOKE_RIDGE_INSET = 2;

/**
 * Smoke flip-book bake scale — draw resolution only. On-screen size is
 * SMOKE_SPRITE_SCALE. (Bake 0.3 + tight STEP-1b radii + bounds-based capture
 * collapsed the band to a 2×13 sliver.)
 */
export const SMOKE_BAKE_SCALE = 1.0;
/**
 * Fixed generateTexture frame (px at SMOKE_BAKE_SCALE). Sized to hold a filled
 * column of separated puffs (STEP 1d ink-fill), not a hairline with padding.
 */
export const SMOKE_TEX_WIDTH = 48;
export const SMOKE_TEX_HEIGHT = 96;
/**
 * Crowd smoke sprite scale. Visible plume ≈ inkBBox × scale.
 * With ~70–90%×48 / ~100%×96 ink fill → on-screen ≈ 23–31 × 66–67 at zoom 1
 * (mid building plume: readable column, not a facade veil / not a thread).
 */
export const SMOKE_SPRITE_SCALE = 0.7;
/** Phase base so every baked frame has a populated column (~5 alive puffs). */
const SMOKE_BAKE_T0 = 2.0;
/** Flip-book cycle length (must match bakeSmokeBand). */
const SMOKE_CYCLE_PERIOD = 1.4;
/** Minimum lobe alpha counted as visible soot for ink-coverage measurement. */
export const SMOKE_INK_ALPHA_FLOOR = 0.08;

const FIRE_FRAMES = 8;
const SMOKE_FRAMES = 6;

export { SEVERITY_SPAWN_MULTIPLIER, SEVERITY_SCALE, SEVERITY_RANK };

// ---- Deterministic seeded helpers (NO Math.random) ----

/** Deterministic seeded phase [0, 100) from fileId. */
export function seededPhase(fileId: string): number {
  const h = hashString(fileId);
  return (h % 9973) / 9973 * 100;
}

/** Fast mulberry32 PRNG from a 32-bit seed — deterministic. */
function xmur3Fast(seed: number): () => number {
  let a = (seed >>> 0) | 0;
  return () => {
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

// ---- Minimal renderer interface (same pattern as TextureSource in buildingAtlas.ts) ----

export interface FireTextureSource {
  generateTexture(options: {
    target: Container;
    resolution?: number;
    antialias?: boolean;
    /** When set, bake this region instead of local bounds (smoke band pad). */
    frame?: Rectangle;
  }): Texture;
}

// ---- Procedural draw helpers (reproduce kitcd/anims Flame/Smoke art) ----

function applyTint(baseColor: number, tint: number): number {
  if (tint === 0xffffff) return baseColor;
  const r = Math.min(255, ((baseColor >> 16) & 0xff) * ((tint >> 16) & 0xff) / 255) | 0;
  const g = Math.min(255, ((baseColor >> 8) & 0xff) * ((tint >> 8) & 0xff) / 255) | 0;
  const b = Math.min(255, (baseColor & 0xff) * (tint & 0xff) / 255) | 0;
  return (r << 16) | (g << 8) | b;
}

function drawFlameFrame(g: Graphics, t: number, scale: number, tint: number): void {
  const s = scale;
  const fl = 0.9 + Math.sin(t * 11) * 0.14 + Math.sin(t * 23) * 0.06;
  const sway = Math.sin(t * 7) * 2.6 * s;
  const sway2 = Math.sin(t * 13 + 1) * 2 * s;

  g.clear();
  g.ellipse(0, -4 * s, 8.5 * s, 5 * s).fill({ color: applyTint(0xb23a1e, tint), alpha: 0.9 });
  g.ellipse(sway, -15 * s * fl, 9 * s, 19 * s * fl).fill({ color: applyTint(0xe8541f, tint), alpha: 0.96 });
  g.ellipse(sway2 + 3 * s, -13 * s * fl, 4.5 * s, 13 * s * fl).fill({ color: applyTint(0xf2731f, tint), alpha: 0.9 });
  g.ellipse(sway * 0.7, -16 * s * fl, 5.6 * s, 14 * s * fl).fill({ color: applyTint(0xf7a024, tint), alpha: 0.97 });
  g.ellipse(sway * 0.5, -14 * s * fl, 3 * s, 9 * s * fl).fill({ color: applyTint(0xffe7a0, tint), alpha: 1 });
  g.ellipse(sway * 0.4, -11 * s * fl, 1.6 * s, 5 * s * fl).fill({ color: applyTint(0xfff6da, tint), alpha: 1 });
  g.circle(0, -11 * s, 30 * s).fill({ color: applyTint(0xf2922e, tint), alpha: 0.2 + 0.07 * Math.sin(t * 9) });
}

/** One filled circle lobe in draw-space (matches drawSmokeFrame fills). */
export interface SmokeLobe {
  x: number;
  y: number;
  r: number;
  a: number;
}

/** One puff envelope (center + design radius) — for spacing / variation tests. */
export interface SmokePuffGeom {
  x: number;
  y: number;
  /** Design radius (r0 + age × growth) × scale — lobe hull extends ~1.15× this. */
  r: number;
  /** Main-body lobe alpha at this age. */
  a: number;
  age: number;
}

/**
 * Vertical spacing between consecutive puff centres (draw-space, scale 1).
 * spacing = interval × rate × rise. Overlap when spacing < r_i + r_j.
 */
export function smokePuffVerticalSpacing(scale: number = 1): number {
  return SMOKE_PUFF.interval * SMOKE_PUFF.rate * SIN_SMOKE.rise * scale;
}

/** Internal puff sample including the seeded lobe rotation angle. */
interface SmokePuffSample extends SmokePuffGeom {
  ang0: number;
}

/**
 * Shared spawn loop for puff envelopes (and lobe expansion).
 * Wind is shared per frame so the column leans as one; wobble is per-puff.
 */
function sampleSmokePuffs(t: number, scale: number): SmokePuffSample[] {
  const s = scale;
  const puffs: SmokePuffSample[] = [];
  const { interval, rate, r0Min, r0Span, growth, pxHalf } = SMOKE_PUFF;
  // Shared wind phase for this frame — tilts the whole column as one lean.
  const wind = Math.sin(t * 1.7) * SIN_SMOKE.driftSpan;

  for (let spawnT = t; spawnT >= 0; spawnT -= interval) {
    const age = (t - spawnT) * rate;
    if (age > 1) continue;

    const seed = Math.floor(spawnT * 1000);
    const rng = xmur3Fast(seed);
    const px = (rng() * (pxHalf * 2) - pxHalf) * s;
    // Small per-puff wobble on top of the shared wind lean (not full scatter).
    const wobble = (rng() * 2 - 1) * SIN_SMOKE.driftSpan * 0.22 * s;
    const r0 = r0Min + rng() * r0Span;
    const ang0 = rng() * Math.PI * 2;

    const y = -age * SIN_SMOKE.rise * s;
    const x = px + wind * age * s + wobble * age;
    const r = (r0 + age * growth) * s;
    // Fade the top, weight the base — still dissolves upward, stays above ink floor longer.
    const fade = Math.pow(1 - age, 0.95);
    const a = SIN_SMOKE.baseAlpha * SIN_SMOKE.lobeAlphaWeight * fade;

    puffs.push({ x, y, r, a, age, ang0 });
  }
  return puffs;
}

/**
 * Enumerate puff envelopes alive at time `t` (centers + design radii).
 * Same spawn loop as drawSmokeFrame / enumerateSmokeLobes.
 */
export function enumerateSmokePuffs(t: number, scale: number): SmokePuffGeom[] {
  return sampleSmokePuffs(t, scale).map(({ x, y, r, a, age }) => ({
    x,
    y,
    r,
    a,
    age,
  }));
}

/**
 * Expand one puff into offset lobes of *different* radii (bumpy silhouette).
 * Not concentric rings — satellites sit off-centre so the union is irregular.
 */
function lobesForPuff(
  x: number,
  y: number,
  r: number,
  a: number,
  ang0: number,
): SmokeLobe[] {
  // Satellite defs: unit offsets × r, radius fraction, alpha fraction of main.
  const sats: { ox: number; oy: number; rr: number; aa: number }[] = [
    { ox: 0.52, oy: -0.38, rr: 0.68, aa: 0.82 },
    { ox: -0.48, oy: 0.22, rr: 0.52, aa: 0.72 },
    { ox: 0.18, oy: 0.55, rr: 0.58, aa: 0.68 },
    { ox: -0.32, oy: -0.48, rr: 0.42, aa: 0.55 },
  ];
  const c = Math.cos(ang0);
  const sn = Math.sin(ang0);
  const out: SmokeLobe[] = [{ x, y, r: r * 0.7, a }];
  for (const L of sats) {
    const ox = (L.ox * c - L.oy * sn) * r;
    const oy = (L.ox * sn + L.oy * c) * r;
    out.push({
      x: x + ox,
      y: y + oy,
      r: r * L.rr,
      a: a * L.aa,
    });
  }
  return out;
}

/**
 * Enumerate every soot lobe alive at time `t` (same params as drawSmokeFrame).
 * Analytical stand-in for GPU readback — used by ink-coverage tests.
 */
export function enumerateSmokeLobes(t: number, scale: number): SmokeLobe[] {
  const lobes: SmokeLobe[] = [];
  for (const p of sampleSmokePuffs(t, scale)) {
    const puffLobes = lobesForPuff(p.x, p.y, p.r, p.a, p.ang0);
    for (const L of puffLobes) lobes.push(L);
  }
  return lobes;
}

/** Bake-frame rectangle in draw-space at the given bake scale. */
export function smokeBakeFrameRect(scale: number): {
  x: number;
  y: number;
  w: number;
  h: number;
} {
  const s = scale / SMOKE_BAKE_SCALE;
  return {
    x: (-SMOKE_TEX_WIDTH / 2) * s,
    y: (-SMOKE_TEX_HEIGHT + 8) * s,
    w: SMOKE_TEX_WIDTH * s,
    h: SMOKE_TEX_HEIGHT * s,
  };
}

export interface SmokeInkCoverage {
  /** Alpha-nonzero bounding-box width / frame width. */
  widthRatio: number;
  /** Alpha-nonzero bounding-box height / frame height. */
  heightRatio: number;
  /** Fraction of bbox samples with any lobe alpha ≥ floor. */
  fillRatio: number;
  inkW: number;
  inkH: number;
  frameW: number;
  frameH: number;
  lobeCount: number;
  puffCount: number;
}

/**
 * Measure how much of the bake frame is filled by drawn soot (analytical).
 * Samples the frame grid; a sample is ink if any lobe covers it with a ≥ floor.
 * This is the number that predicts on-screen visibility — not frame size alone.
 */
export function measureSmokeInkCoverage(
  t: number,
  scale: number = SMOKE_BAKE_SCALE,
  alphaFloor: number = SMOKE_INK_ALPHA_FLOOR,
): SmokeInkCoverage {
  const frame = smokeBakeFrameRect(scale);
  const allLobes = enumerateSmokeLobes(t, scale);
  // Lobe groups of SMOKE_LOBES_PER_PUFF; count before alpha floor culls faded tips.
  const puffCount = Math.floor(allLobes.length / SMOKE_LOBES_PER_PUFF);
  const lobes = allLobes.filter((L) => L.a >= alphaFloor && L.r > 0);

  // 1px grid over the frame (integer sample centers).
  const step = 1;
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  let inkSamples = 0;
  const cols = Math.max(1, Math.ceil(frame.w / step));
  const rows = Math.max(1, Math.ceil(frame.h / step));

  for (let iy = 0; iy < rows; iy++) {
    const py = frame.y + (iy + 0.5) * step;
    for (let ix = 0; ix < cols; ix++) {
      const px = frame.x + (ix + 0.5) * step;
      let hit = false;
      for (const L of lobes) {
        const dx = px - L.x;
        const dy = py - L.y;
        if (dx * dx + dy * dy <= L.r * L.r) {
          hit = true;
          break;
        }
      }
      if (hit) {
        inkSamples++;
        if (px < minX) minX = px;
        if (px > maxX) maxX = px;
        if (py < minY) minY = py;
        if (py > maxY) maxY = py;
      }
    }
  }

  if (inkSamples === 0) {
    return {
      widthRatio: 0,
      heightRatio: 0,
      fillRatio: 0,
      inkW: 0,
      inkH: 0,
      frameW: frame.w,
      frameH: frame.h,
      lobeCount: lobes.length,
      puffCount,
    };
  }

  // BBox of ink samples (sample centers → expand half-step to pixel extent).
  const inkW = maxX - minX + step;
  const inkH = maxY - minY + step;
  // Re-count fill inside the ink bbox only.
  let bboxSamples = 0;
  let bboxInk = 0;
  const bx0 = minX - step * 0.5;
  const by0 = minY - step * 0.5;
  const bCols = Math.max(1, Math.ceil(inkW / step));
  const bRows = Math.max(1, Math.ceil(inkH / step));
  for (let iy = 0; iy < bRows; iy++) {
    const py = by0 + (iy + 0.5) * step;
    for (let ix = 0; ix < bCols; ix++) {
      const px = bx0 + (ix + 0.5) * step;
      // Stay inside frame
      if (px < frame.x || px >= frame.x + frame.w || py < frame.y || py >= frame.y + frame.h) {
        continue;
      }
      bboxSamples++;
      for (const L of lobes) {
        const dx = px - L.x;
        const dy = py - L.y;
        if (dx * dx + dy * dy <= L.r * L.r) {
          bboxInk++;
          break;
        }
      }
    }
  }

  return {
    widthRatio: inkW / frame.w,
    heightRatio: inkH / frame.h,
    fillRatio: bboxSamples > 0 ? bboxInk / bboxSamples : 0,
    inkW,
    inkH,
    frameW: frame.w,
    frameH: frame.h,
    lobeCount: lobes.length,
    puffCount,
  };
}

/**
 * Bake times used for the six smoke flip-book frames (matches bakeSmokeBand).
 */
export function smokeBakeTimes(): number[] {
  const times: number[] = [];
  for (let i = 0; i < SMOKE_FRAMES; i++) {
    times.push(SMOKE_BAKE_T0 + (i / SMOKE_FRAMES) * SMOKE_CYCLE_PERIOD);
  }
  return times;
}

/**
 * Sin-smoke flip-book frame: overlapping multi-lobe puffs that merge into a
 * continuous, irregular column (STEP 3 — not beads on a string). ~5–6 alive.
 * STEP 1d: puff radii fill the bake band (see measureSmokeInkCoverage).
 */
function drawSmokeFrame(g: Graphics, t: number, scale: number): void {
  g.clear();
  const lobes = enumerateSmokeLobes(t, scale);
  // Cycle sooty core / mid / edge tints across irregular lobes.
  const colors = [SIN_SMOKE.colorCore, SIN_SMOKE.colorMid, SIN_SMOKE.colorEdge];
  for (let i = 0; i < lobes.length; i++) {
    const L = lobes[i];
    g.circle(L.x, L.y, L.r).fill({ color: colors[i % 3], alpha: L.a });
  }
}

// ---- Atlas bake ----

export interface FireAtlas {
  flames: Record<FireSeverity, Texture[]>;
  smokes: Texture[];
}

/**
 * A7 — resolve the real flip-book frames (`fx:fire:f0..f7`) from the sprite
 * bank. All-or-nothing (a partial set would stutter the cycle); null keeps the
 * procedural flame bands. Only FIRE_FRAMES (8) of the strip's 9 frames are
 * used so every consumer's `% FIRE_FRAMES` indexing stays valid.
 */
function resolveFireArt(
  bank: { get(key: string): Texture | null } | null | undefined,
): Texture[] | null {
  if (!bank) return null;
  const frames: Texture[] = [];
  for (let i = 0; i < FIRE_FRAMES; i++) {
    const tex = bank.get(`fx:fire:f${i}`);
    if (!tex) return null;
    frames.push(tex);
  }
  return frames;
}

/**
 * Bake fire + smoke flip-book textures. Call ONCE per session (in PolisRenderer
 * constructor) with the real PIXI renderer. Matches the buildingAtlas generateTexture
 * pattern: draw Graphics off-screen, call renderer.generateTexture, destroy Graphics.
 *
 * A7 — with a sprite bank carrying the OGA 9-frame fire strip, the flame bands
 * re-bake the REAL pixel-art frames per severity (scale + tint) instead of the
 * procedural ellipses; the atlas contract (frame counts, ownership, destroy
 * path) is identical either way. Smoke stays procedural (the strip has none).
 */
export function bakeFireAtlas(
  renderer: FireTextureSource,
  bank?: { get(key: string): Texture | null } | null,
): FireAtlas {
  const art = resolveFireArt(bank);
  // Band scales match the procedural bands' on-screen heights: a procedural
  // flame at band scale s is ~60s px tall; the art frame's flame is ~56px.
  const flames = art
    ? {
        smoke: bakeFlameBandFromArt(renderer, art, 0.24, 0xffffff),
        fire: bakeFlameBandFromArt(renderer, art, 0.43, 0xffffff),
        // Milder red than the procedural 0xcc3322 — the pixel art is already
        // saturated; a hard multiply would crush it toward black.
        inferno: bakeFlameBandFromArt(renderer, art, 0.64, 0xffb0a0),
      }
    : {
        smoke: bakeFlameBand(renderer, 0.22, 0xffffff),
        fire: bakeFlameBand(renderer, 0.4, 0xffffff),
        inferno: bakeFlameBand(renderer, 0.6, 0xcc3322),
      };
  // Bake at SMOKE_BAKE_SCALE for texture resolution; sprites use
  // SMOKE_SPRITE_SCALE for world size (see createCrowdFire).
  const smokes = bakeSmokeBand(renderer, SMOKE_BAKE_SCALE);
  return { flames, smokes };
}

/**
 * Bake one severity band from the real strip frames: each frame is drawn
 * through an off-screen Sprite (scaled + tinted) into its own RenderTexture,
 * so the returned textures are OWNED by the atlas exactly like the procedural
 * ones (destroyFireAtlas destroys them; the bank's source frames are never
 * destroyed here).
 */
function bakeFlameBandFromArt(
  renderer: FireTextureSource,
  art: Texture[],
  scale: number,
  tint: number,
): Texture[] {
  const frames: Texture[] = [];
  const sp = new Sprite();
  sp.tint = tint;
  sp.scale.set(scale);
  const container = new Container();
  container.addChild(sp);
  for (const frameTex of art) {
    sp.texture = frameTex;
    const baked = renderer.generateTexture({
      target: container,
      resolution: 1,
      antialias: false,
    });
    // MAX-RECALL fix — the source is PIXEL ART: under the default linear
    // filter, magnification at high zoom (MAX_ZOOM 3) smears the chunky
    // texels into mush. Nearest keeps the crunch; the procedural bands stay
    // linear (smooth vector shapes benefit from it). Guarded: unit tests
    // bake through a stub renderer whose textures have no source.
    if (baked.source) baked.source.scaleMode = "nearest";
    frames.push(baked);
  }
  sp.destroy(); // texture stays owned by the bank
  container.destroy({ children: false });
  return frames;
}

function bakeFlameBand(renderer: FireTextureSource, scale: number, tint: number): Texture[] {
  const frames: Texture[] = [];
  const cyclePeriod = 0.7; // one anim cycle
  const g = new Graphics();
  const container = new Container();
  container.addChild(g);

  for (let i = 0; i < FIRE_FRAMES; i++) {
    const t = (i / FIRE_FRAMES) * cyclePeriod;
    drawFlameFrame(g, t, scale, tint);
    const tex = renderer.generateTexture({ target: container, resolution: 1, antialias: false });
    frames.push(tex);
  }

  g.destroy();
  container.destroy({ children: false });
  return frames;
}

function bakeSmokeBand(renderer: FireTextureSource, scale: number): Texture[] {
  const frames: Texture[] = [];
  const g = new Graphics();
  const container = new Container();
  container.addChild(g);
  // Fixed frame in draw-space (scale-relative): width/height match the exported
  // SMOKE_TEX_* at scale === SMOKE_BAKE_SCALE. STEP 1d fills this band with
  // soot (not a hairline padded into a large transparent frame).
  const fr = smokeBakeFrameRect(scale);
  const frame = new Rectangle(fr.x, fr.y, fr.w, fr.h);

  // smokeBakeTimes() already offsets past the empty birth window.
  for (const t of smokeBakeTimes()) {
    drawSmokeFrame(g, t, scale);
    const tex = renderer.generateTexture({
      target: container,
      resolution: 1,
      antialias: false,
      frame,
    });
    frames.push(tex);
  }

  g.destroy();
  container.destroy({ children: false });
  return frames;
}

/** Destroy all atlas textures (called in PolisRenderer.destroy). */
export function destroyFireAtlas(atlas: FireAtlas): void {
  for (const sev of ["smoke", "fire", "inferno"] as FireSeverity[]) {
    for (const t of atlas.flames[sev]) t.destroy(true);
  }
  for (const t of atlas.smokes) t.destroy(true);
}

// ---- Tier F1: crowd fire Sprite ----

export interface CrowdFire {
  fireSprite: Sprite;
  smokeSprite: Sprite;
  phase: number;
  severity: FireSeverity;
  lastFireFrame: number;
  lastSmokeFrame: number;
}

/**
 * @param x Building iso foot X (front-bottom anchor).
 * @param y Building iso foot Y (front-bottom anchor).
 * @param bodyHeightPx Silhouette height above the iso foot — same quantity as
 *   `labelDepth` / `makeLabel`'s `depthPx`. Smoke anchors at the roof line;
 *   flame stays near the body. When 0 (tests without height), smoke gets a
 *   small lift above the flame only.
 */
export function createCrowdFire(
  atlas: FireAtlas,
  fileId: string,
  severity: FireSeverity,
  x: number,
  y: number,
  bodyHeightPx = 0,
): CrowdFire {
  const phase = seededPhase(fileId);
  const fireIdx = Math.abs(Math.floor(phase)) % FIRE_FRAMES;
  const smokeIdx = Math.abs(Math.floor(phase / 7)) % SMOKE_FRAMES;

  // Fire band only used when severity shows flame; smoke-severity still picks a
  // texture so a later severity upgrade can flip the sprite without rebuild.
  const fireSprite = new Sprite(atlas.flames[severity === "smoke" ? "fire" : severity][fireIdx]);
  fireSprite.anchor.set(0.5, 1);
  // Flames belong on the body (near the foot), not on the roof.
  fireSprite.position.set(x, y - CROWD_FLAME_FOOT_LIFT);
  fireSprite.visible = crowdFireShowsFlame(severity);

  const smokeSprite = new Sprite(atlas.smokes[smokeIdx]);
  smokeSprite.anchor.set(0.5, 1);
  // World size is sprite scale, not bake scale — texture stays high-res.
  smokeSprite.scale.set(SMOKE_SPRITE_SCALE);
  // Roof origin: same height basis as the file label (`-depthPx`), inset a hair
  // below the ridge so the column reads as lifting off the building.
  const smokeY =
    bodyHeightPx > 0
      ? y - bodyHeightPx + CROWD_SMOKE_RIDGE_INSET
      : y - CROWD_FLAME_FOOT_LIFT - (severity === "smoke" ? 6 : 10);
  smokeSprite.position.set(x, smokeY);

  return { fireSprite, smokeSprite, phase, severity, lastFireFrame: fireIdx, lastSmokeFrame: smokeIdx };
}

export function stepCrowdFire(
  cf: CrowdFire,
  atlas: FireAtlas,
  stepFrame: number,
  halfRate: boolean,
): void {
  if (halfRate && stepFrame % 2 !== 0) return;
  const fireIdx = (stepFrame + Math.floor(cf.phase)) % FIRE_FRAMES;
  const smokeIdx = (stepFrame + Math.floor(cf.phase / 3)) % SMOKE_FRAMES;
  if (fireIdx !== cf.lastFireFrame) {
    cf.fireSprite.texture = atlas.flames[cf.severity][fireIdx];
    cf.lastFireFrame = fireIdx;
  }
  if (smokeIdx !== cf.lastSmokeFrame) {
    cf.smokeSprite.texture = atlas.smokes[smokeIdx];
    cf.lastSmokeFrame = smokeIdx;
  }
}

// ---- Tier F2: hero fire ParticleContainer pool ----

const MAX_FLAMES = 40;
const MAX_EMBERS = 12;
const MAX_SMOKE = 10;
const TOTAL_PARTICLES = MAX_FLAMES + MAX_EMBERS + MAX_SMOKE;

interface HeroParticleState {
  /** Dead slots (beyond the seeded count) are skipped by stepHeroFire. */
  active: boolean;
  life: number;
  lifeRate: number;
  baseX: number;
  baseY: number;
  driftX: number;
  driftY: number;
  scaleBase: number;
  alphaBase: number;
}

export interface HeroFire {
  container: ParticleContainer;
  targetFileId: string | null;
  crossfade: number;
  crossfading: boolean;
  crossfadeDirection: -1 | 0 | 1;
  flameParticles: Particle[];
  emberParticles: Particle[];
  smokeParticles: Particle[];
  particleState: HeroParticleState[];
}

let sharedParticleTexture: Texture | null = null;

function getParticleTexture(renderer: FireTextureSource): Texture {
  if (sharedParticleTexture) return sharedParticleTexture;
  const g = new Graphics();
  g.circle(8, 8, 8).fill({ color: 0xffffff, alpha: 0.9 });
  g.circle(8, 8, 5).fill({ color: 0xffffff, alpha: 1 });
  const container = new Container();
  container.addChild(g);
  sharedParticleTexture = renderer.generateTexture({ target: container, resolution: 1, antialias: false });
  g.destroy();
  container.destroy({ children: false });
  return sharedParticleTexture;
}

export function createHeroFire(
  renderer: FireTextureSource,
  fileId: string,
  
  x: number,
  y: number,
): HeroFire {
  const texture = getParticleTexture(renderer);

  const container = new ParticleContainer({
    texture,
    dynamicProperties: {
      position: true,
      scale: true,
      vertex: false,
      rotation: false,
      uvs: false,
      color: true,
    },
  });

  const seed = seededPhase(fileId);
  const rngSeq = xmur3Fast(Math.floor(seed * 1e6));

  const flameParticles: Particle[] = [];
  const emberParticles: Particle[] = [];
  const smokeParticles: Particle[] = [];
  const particleState: HeroParticleState[] = [];

  const flameCount = Math.round(28 + rngSeq() * 12);
  for (let i = 0; i < MAX_FLAMES; i++) {
    const p = new Particle(texture);
    p.x = x; p.y = y;
    p.alpha = i < flameCount ? 1 : 0; // Particle has no `visible`; alpha 0 = inert (state.life 0 keeps it dead)
    p.tint = i % 3 === 0 ? 0xffe7a0 : i % 3 === 1 ? 0xf7a024 : 0xe8541f;
    container.addParticle(p);
    flameParticles.push(p);
    particleState.push({
      active: i < flameCount,
      life: i < flameCount ? rngSeq() : 0,
      lifeRate: 0.6 + rngSeq() * 0.8,
      baseX: x, baseY: y,
      driftX: (rngSeq() * 2 - 1) * 30,
      driftY: -(20 + rngSeq() * 40),
      scaleBase: 0.4 + rngSeq() * 0.6,
      alphaBase: 0.7 + rngSeq() * 0.3,
    });
  }

  const emberCount = Math.round(8 + rngSeq() * 4);
  for (let i = 0; i < MAX_EMBERS; i++) {
    const p = new Particle(texture);
    p.x = x; p.y = y;
    p.alpha = i < emberCount ? 1 : 0; // Particle has no `visible`; alpha 0 = inert (state.life 0 keeps it dead)
    p.tint = i % 2 === 0 ? 0xb23a1e : 0xf2731f;
    p.scaleX = 0.3; p.scaleY = 0.3;
    container.addParticle(p);
    emberParticles.push(p);
    particleState.push({
      active: i < emberCount,
      life: i < emberCount ? rngSeq() : 0,
      lifeRate: 0.8 + rngSeq() * 1.2,
      baseX: x, baseY: y,
      driftX: (rngSeq() * 2 - 1) * 40,
      driftY: -(10 + rngSeq() * 30),
      scaleBase: 0.2 + rngSeq() * 0.3,
      alphaBase: 0.6 + rngSeq() * 0.4,
    });
  }

  const smokeCount = Math.round(6 + rngSeq() * 4);
  for (let i = 0; i < MAX_SMOKE; i++) {
    const p = new Particle(texture);
    p.x = x; p.y = y;
    p.alpha = i < smokeCount ? 1 : 0; // Particle has no `visible`; alpha 0 = inert (state.life 0 keeps it dead)
    p.tint = SIN_SMOKE.colorMid;
    p.scaleX = 0.6; p.scaleY = 0.6;
    container.addParticle(p);
    smokeParticles.push(p);
    particleState.push({
      active: i < smokeCount,
      life: i < smokeCount ? rngSeq() : 0,
      lifeRate: 0.3 + rngSeq() * 0.5,
      baseX: x, baseY: y,
      driftX: (rngSeq() * 2 - 1) * 20,
      driftY: -(30 + rngSeq() * 50),
      scaleBase: 0.4 + rngSeq() * 0.4,
      alphaBase: 0.3 + rngSeq() * 0.2,
    });
  }

  return {
    container,
    targetFileId: fileId,
    crossfade: 0,
    crossfading: true,
    crossfadeDirection: 1,
    flameParticles,
    emberParticles,
    smokeParticles,
    particleState,
  };
}

export function retargetHeroFire(
  hf: HeroFire,
  fileId: string,
  severity: FireSeverity,
  x: number,
  y: number,
): void {
  hf.targetFileId = fileId;
  const seed = seededPhase(fileId);
  const rngSeq = xmur3Fast(Math.floor(seed * 1e6));

  const flameBase = Math.round(28 + rngSeq() * 12);
  const mult = SEVERITY_SPAWN_MULTIPLIER[severity];
  // Smoke-severity heroes: soot only — no flame/ember particles.
  const showsFlame = crowdFireShowsFlame(severity);
  const flameCount = showsFlame
    ? Math.min(MAX_FLAMES, Math.round(flameBase * mult))
    : 0;
  const emberCount = showsFlame
    ? Math.min(MAX_EMBERS, Math.round((8 + rngSeq() * 4) * mult))
    : 0;
  // Smoke-severity gets a fuller soot column; fire/inferno keep the lighter cap.
  const smokeCount = Math.min(
    MAX_SMOKE,
    Math.round((showsFlame ? 6 + rngSeq() * 4 : 8 + rngSeq() * 2) * mult),
  );
  const scaleExtra = SEVERITY_SCALE[severity];

  const resetParticle = (
    ps: HeroParticleState[], particles: Particle[], offset: number, count: number, max: number,
    lifeRateMin: number, lifeRateRange: number,
    driftXRange: number, driftYMin: number, driftYRange: number,
    scaleBaseMin: number, scaleBaseRange: number,
    alphaBaseMin: number, alphaBaseRange: number,
  ) => {
    for (let i = 0; i < max; i++) {
      const p = particles[i];
      const vis = i < count;
      p.alpha = vis ? 1 : 0; // Particle has no `visible`; inactive slots stay dead
      p.x = x; p.y = y;
      ps[offset + i] = {
        active: vis,
        life: vis ? rngSeq() : 0,
        lifeRate: lifeRateMin + rngSeq() * lifeRateRange,
        baseX: x, baseY: y,
        driftX: (rngSeq() * 2 - 1) * driftXRange,
        driftY: -(driftYMin + rngSeq() * driftYRange),
        scaleBase: (scaleBaseMin + rngSeq() * scaleBaseRange) * scaleExtra,
        alphaBase: alphaBaseMin + rngSeq() * alphaBaseRange,
      };
    }
  };

  resetParticle(hf.particleState, hf.flameParticles, 0, flameCount, MAX_FLAMES,
    0.6, 0.8, 30, 20, 40, 0.4, 0.6, 0.7, 0.3);
  resetParticle(hf.particleState, hf.emberParticles, MAX_FLAMES, emberCount, MAX_EMBERS,
    0.8, 1.2, 40, 10, 30, 0.2, 0.3, 0.6, 0.4);
  resetParticle(hf.particleState, hf.smokeParticles, MAX_FLAMES + MAX_EMBERS, smokeCount, MAX_SMOKE,
    0.3, 0.5, 20, 30, 50, 0.4, 0.4, 0.3, 0.2);

  hf.crossfade = 0;
  hf.crossfading = true;
  hf.crossfadeDirection = 1;
  hf.container.alpha = 0.3;
  hf.container.visible = true;
  hf.container.update();
}

export function stepHeroFire(hf: HeroFire, dt: number): void {
  if (hf.crossfading) {
    const fadeSpeed = 1 / 0.3;
    hf.crossfade += dt * fadeSpeed * hf.crossfadeDirection;
    if (hf.crossfade >= 1) {
      hf.crossfade = 1; hf.crossfading = false; hf.crossfadeDirection = 0;
    } else if (hf.crossfade <= 0) {
      // Demotion completed — park the hero fire so the pool slot is reused.
      // Cannot check crossfadeDirection from outside (it gets zeroed here),
      // so we park while we still know it was a demotion.
      hf.crossfade = 0; hf.crossfading = false; hf.crossfadeDirection = 0;
      if (hf.targetFileId !== null) {
        parkHeroFire(hf);
        return; // no particle step — container is hidden
      }
    }
    hf.container.alpha = 0.3 + hf.crossfade * 0.7;
  }

  // Allocation-free: walk the three pools by offset (ticker rule).
  const nf = hf.flameParticles.length;
  const ne = hf.emberParticles.length;
  for (let i = 0; i < TOTAL_PARTICLES; i++) {
    const st = hf.particleState[i];
    if (!st || !st.active) continue;
    const p =
      i < nf ? hf.flameParticles[i]
      : i < nf + ne ? hf.emberParticles[i - nf]
      : hf.smokeParticles[i - nf - ne];
    if (!p) continue;
    st.life += dt * st.lifeRate;
    if (st.life >= 1) st.life = 0;
    const t = st.life;
    p.x = st.baseX + st.driftX * t;
    p.y = st.baseY + st.driftY * t;
    const sc = st.scaleBase * (1 - t * 0.6);
    p.scaleX = sc; p.scaleY = sc;
    p.alpha = Math.max(0, Math.min(1, st.alphaBase * (1 - t)));
  }
  hf.container.update();
}

export function parkHeroFire(hf: HeroFire): void {
  hf.targetFileId = null;
  hf.container.visible = false;
  hf.container.alpha = 0;
  hf.crossfade = 0;
  hf.crossfading = false;
  hf.crossfadeDirection = 0;
}

export function beginDemotionCrossfade(hf: HeroFire): void {
  hf.crossfade = 1;
  hf.crossfading = true;
  hf.crossfadeDirection = -1;
}

// ---- Promotion ranking (pure, testable) ----

export interface PromotableBuilding {
  fileId: string;
  severity: FireSeverity;
  distToCenter: number;
}

export function rankForPromotion(buildings: PromotableBuilding[]): PromotableBuilding[] {
  return [...buildings].sort((a, b) => {
    const sevDiff = SEVERITY_RANK[b.severity] - SEVERITY_RANK[a.severity];
    if (sevDiff !== 0) return sevDiff;
    return a.distToCenter - b.distToCenter;
  });
}
