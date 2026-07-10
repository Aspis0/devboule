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

function drawSmokeFrame(g: Graphics, t: number, scale: number): void {
  const s = scale;
  g.clear();
  const puffInterval = 0.28;
  const puffRate = 0.42;
  // Walk backwards from t to find all puffs alive at this time.
  for (let spawnT = t; spawnT >= 0; spawnT -= puffInterval) {
    const age = (t - spawnT) * puffRate;
    if (age > 1) continue;

    const seed = Math.floor(spawnT * 1000);
    const rng = xmur3Fast(seed);
    const px = (rng() * 4 - 2) * s;
    const drift = (rng() * 17 - 7) * s;
    const r0 = 3 + rng() * 2;

    const y = -age * 52 * s;
    const x = px + drift * age;
    const r = (r0 + age * 13) * s;
    const a = 0.5 * (1 - age);

    g.circle(x, y, r).fill({ color: 0x7e7868, alpha: a });
    g.circle(x - r * 0.25, y - r * 0.22, r * 0.62).fill({ color: 0x9c968a, alpha: a * 0.8 });
    g.circle(x - r * 0.4, y - r * 0.35, r * 0.34).fill({ color: 0xb6b0a2, alpha: a * 0.5 });
  }
}

// ---- Atlas bake ----

const FIRE_FRAMES = 8;
const SMOKE_FRAMES = 6;

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
  const smokes = bakeSmokeBand(renderer, 0.28);
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
    frames.push(
      renderer.generateTexture({ target: container, resolution: 1, antialias: false }),
    );
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
  const cyclePeriod = 1.4;
  const g = new Graphics();
  const container = new Container();
  container.addChild(g);

  for (let i = 0; i < SMOKE_FRAMES; i++) {
    const t = (i / SMOKE_FRAMES) * cyclePeriod;
    drawSmokeFrame(g, t, scale);
    const tex = renderer.generateTexture({ target: container, resolution: 1, antialias: false });
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

export function createCrowdFire(
  atlas: FireAtlas,
  fileId: string,
  severity: FireSeverity,
  x: number,
  y: number,
): CrowdFire {
  const phase = seededPhase(fileId);
  const fireIdx = Math.abs(Math.floor(phase)) % FIRE_FRAMES;
  const smokeIdx = Math.abs(Math.floor(phase / 7)) % SMOKE_FRAMES;

  const fireSprite = new Sprite(atlas.flames[severity][fireIdx]);
  fireSprite.anchor.set(0.5, 1);
  fireSprite.position.set(x, y);

  const smokeSprite = new Sprite(atlas.smokes[smokeIdx]);
  smokeSprite.anchor.set(0.5, 1);
  smokeSprite.position.set(x, y - 10);

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
    p.tint = 0x7e7868;
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
  const flameCount = Math.min(MAX_FLAMES, Math.round(flameBase * mult));
  const emberCount = Math.min(MAX_EMBERS, Math.round((8 + rngSeq() * 4) * mult));
  const smokeCount = Math.min(MAX_SMOKE, Math.round((6 + rngSeq() * 4) * mult));
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
