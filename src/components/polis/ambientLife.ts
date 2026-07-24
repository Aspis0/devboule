// ambientLife.ts — Phase 4 ambient life: pure planners + PIXI managers.
//
// Systems (each profile-capped, budget-gated, StepClock-stepped):
//   1. Chimney smoke — shared pooled soft puffs on active chimney/workshop/baths
//   2. Civic flags — kit Flag wind phase offset (deterministic per fileId)
//   3. Birds — 0–3 gull silhouettes on bezier arcs (rich only)
//   4. Night windows — additive warm glows; layer alpha = f(darkness)
//   5. Traffic dust — faint motes on trunk roads at far zoom (rich only)
//   6. Forum clusters — static 2–4 figure clusters near civic anchors
//
// HONESTY: all of this is DECORATION. Never invents files/agents; forum clusters
// have no glow/arrow (muted). Chimney smoke is cooler/weaker than disaster fire
// smoke (no orange).
//
// PERF: geometry/textures built once; step only mutates transform/alpha/visible.
// No Math.random — mulberry32 / hashString only. Pure planners are headless-
// testable; PIXI managers are thin wrappers.

import { Container, Graphics, Sprite, Texture } from "pixi.js";
import { hashString, mulberry32 } from "./rng";
import { saltLook } from "./kitcd/buildings";
import { Flag, type AnimInstance } from "./kitcd/anims";
import { drawCitizen, defaultTunic, shadeColor } from "./kitcd/people";
import { DERIVED, PALETTE, tierRank } from "./palette";
import { darken, lighten } from "./iso";
import type { Building } from "../../types/city";
import type { RenderProfile } from "./renderProfile";
import type { BudgetRung } from "./effectsBudget";
import { ambientLifeGates } from "./effectsBudget";
import { MAT } from "./kitcd/iso";

// ---------------------------------------------------------------------------
// Shared constants
// ---------------------------------------------------------------------------

/** Activity window for "recently modified" (ms). ~48h. */
export const RECENT_MODIFIED_MS = 48 * 60 * 60 * 1000;

/** Civic purposes that receive flags. */
export const CIVIC_FLAG_PURPOSES = Object.freeze([
  "townhall",
  "market",
  "theater",
  "library",
  "temple",
] as const);

/** Purposes that emit chimney smoke regardless of saltLook.hasChimney. */
export const ALWAYS_CHIMNEY_PURPOSES = Object.freeze([
  "workshop",
  "baths",
] as const);

/** Forum anchor purposes (static crowd clusters). */
export const FORUM_PURPOSES = Object.freeze([
  "market",
  "townhall",
] as const);

/** Fixed seed for bird PRNG stream (not session-stable required; no Math.random). */
export const BIRDS_SEED = 0xb1d5_c0de;

/** Day-phase darkness threshold above which night windows read. */
export const NIGHT_WINDOW_DARKNESS_THRESHOLD = 0.35;

// Soft cool blue-gray smoke (distinct from warm disaster fire smoke).
const CHIMNEY_SMOKE_TINT = darken(DERIVED.smokeCool, 0.06);
// Night window glow (warm gold — already on-palette).
const WINDOW_GLOW_TINT = DERIVED.windowLit;
// Traffic dust (faint sand).
const DUST_TINT = lighten(PALETTE.sandDark, 0.18);
// Bird silhouette.
const BIRD_TINT = darken(PALETTE.shadow, 0.25);
// Forum figures: muted citizen tunics (baked to shared textures once).
const FORUM_TUNIC_A = shadeColor(defaultTunic("citizen"), 0.82);
const FORUM_TUNIC_B = shadeColor(defaultTunic("watercarrier"), 0.78);
const FORUM_TUNIC_C = shadeColor(defaultTunic("merchant"), 0.8);
/** Max figures per forum cluster (selectForumClusters uses 2..4). */
const FORUM_MAX_FIGURES = 4;

// ---------------------------------------------------------------------------
// Texture source (same narrow surface as fire/atlas — headless-injectable)
// ---------------------------------------------------------------------------

export interface AmbientTextureSource {
  generateTexture(options: {
    target: Container;
    resolution?: number;
    antialias?: boolean;
  }): Texture;
}

// ---------------------------------------------------------------------------
// Pure: activity / chimney eligibility
// ---------------------------------------------------------------------------

/**
 * Parse an ISO (or Date-parseable) lastModified string to epoch ms.
 * Returns null when empty/unparseable — caller treats as "not recent".
 * PURE. Compute once at build, never per frame.
 */
export function parseLastModifiedMs(lastModified: string | undefined | null): number | null {
  if (!lastModified || typeof lastModified !== "string") return null;
  const t = Date.parse(lastModified);
  return Number.isFinite(t) ? t : null;
}

/**
 * True when the building is "active" for ambient chimney smoke:
 * agentPresent set, OR lastModified within RECENT_MODIFIED_MS of nowMs.
 * PURE. nowMs is injected (build-time snapshot).
 */
export function isBuildingActive(
  agentPresent: string | undefined | null,
  lastModifiedMs: number | null,
  nowMs: number,
): boolean {
  if (agentPresent != null && agentPresent !== "") return true;
  if (lastModifiedMs == null) return false;
  return nowMs - lastModifiedMs <= RECENT_MODIFIED_MS && nowMs - lastModifiedMs >= 0;
}

/**
 * True when a building may host a chimney-smoke emitter:
 *   (a) saltLook(salt).hasChimney OR purpose is workshop/baths
 *   (b) isBuildingActive(...)
 * PURE.
 */
export function isChimneyEmitterEligible(input: {
  purpose: string;
  salt: number;
  agentPresent?: string | null;
  lastModifiedMs: number | null;
  nowMs: number;
}): boolean {
  const purposeOk = (ALWAYS_CHIMNEY_PURPOSES as readonly string[]).includes(
    input.purpose,
  );
  const hasChimney = purposeOk || saltLook(input.salt).hasChimney;
  if (!hasChimney) return false;
  return isBuildingActive(input.agentPresent, input.lastModifiedMs, input.nowMs);
}

// ---------------------------------------------------------------------------
// Pure: selection helpers (distance-to-center, stable ties)
// ---------------------------------------------------------------------------

export interface WorldPointCandidate {
  fileId: string;
  x: number;
  y: number;
}

/** Squared distance; used for nearest-center ranking. */
export function dist2(ax: number, ay: number, bx: number, by: number): number {
  const dx = ax - bx;
  const dy = ay - by;
  return dx * dx + dy * dy;
}

/**
 * Sort candidates nearest to (cx,cy); stable fileId tie-break; take up to cap.
 * PURE. Returns a new array of fileIds.
 */
export function selectNearestCap(
  candidates: readonly WorldPointCandidate[],
  cap: number,
  cx: number,
  cy: number,
): string[] {
  if (cap <= 0 || candidates.length === 0) return [];
  const ranked = candidates
    .map((c) => ({
      fileId: c.fileId,
      d2: dist2(c.x, c.y, cx, cy),
    }))
    .sort((a, b) => {
      if (a.d2 !== b.d2) return a.d2 - b.d2;
      return a.fileId < b.fileId ? -1 : a.fileId > b.fileId ? 1 : 0;
    });
  const out: string[] = [];
  const seen = new Set<string>();
  for (const r of ranked) {
    if (seen.has(r.fileId)) continue;
    seen.add(r.fileId);
    out.push(r.fileId);
    if (out.length >= cap) break;
  }
  return out;
}

// ---------------------------------------------------------------------------
// Pure: chimney smoke emitter selection
// ---------------------------------------------------------------------------

export interface ChimneyEmitterInput {
  fileId: string;
  purpose: string;
  salt: number;
  agentPresent?: string | null;
  lastModified: string;
  x: number;
  y: number;
}

/**
 * Select chimney-smoke emitters: eligible + nearest-to-center + profile cap.
 * PURE + deterministic given the same inputs (including nowMs).
 */
export function selectChimneyEmitters(
  buildings: readonly ChimneyEmitterInput[],
  cap: number,
  cx: number,
  cy: number,
  nowMs: number,
): string[] {
  if (cap <= 0) return [];
  const eligible: WorldPointCandidate[] = [];
  for (const b of buildings) {
    const lastModifiedMs = parseLastModifiedMs(b.lastModified);
    if (
      !isChimneyEmitterEligible({
        purpose: b.purpose,
        salt: b.salt,
        agentPresent: b.agentPresent,
        lastModifiedMs,
        nowMs,
      })
    ) {
      continue;
    }
    eligible.push({ fileId: b.fileId, x: b.x, y: b.y });
  }
  return selectNearestCap(eligible, cap, cx, cy);
}

// ---------------------------------------------------------------------------
// Pure: civic flags
// ---------------------------------------------------------------------------

/**
 * Deterministic wind phase offset for a civic flag (seconds-ish into Flag.t).
 * Same fileId → same offset; different ids decorrelated.
 * PURE. No Math.random.
 */
export function flagPhaseOffset(fileId: string): number {
  const h = hashString(fileId);
  // Map to [0, 8) matching Flag's historical Math.random()*8 range.
  return ((h % 9973) / 9973) * 8;
}

export interface CivicFlagInput {
  fileId: string;
  purpose: string;
  x: number;
  y: number;
}

/**
 * Select civic buildings that should carry a Flag with deterministic phase.
 * PURE. Cap + nearest-center.
 */
export function selectCivicFlags(
  buildings: readonly CivicFlagInput[],
  cap: number,
  cx: number,
  cy: number,
): string[] {
  if (cap <= 0) return [];
  const civic = buildings.filter((b) =>
    (CIVIC_FLAG_PURPOSES as readonly string[]).includes(b.purpose),
  );
  return selectNearestCap(civic, cap, cx, cy);
}

// ---------------------------------------------------------------------------
// Pure: night window glow selection
// ---------------------------------------------------------------------------

export interface WindowGlowInput {
  fileId: string;
  /** Kit level 0..4 (tierRank). */
  level: number;
  salt: number;
  x: number;
  y: number;
  /** Silhouette depth px (for vertical placement). */
  depth: number;
  /** Footprint half-width px. */
  hw: number;
}

export interface WindowGlowSlot {
  fileId: string;
  /** Local offset from building iso (screen px). */
  ox: number;
  oy: number;
}

/**
 * Local window glow offsets for a building from saltLook.winMode.
 * Level < 2 → empty (mid+ only). winMode drives count/spacing; fallback is a
 * single door-glow when winMode yields nothing (should not happen for level≥2).
 * PURE.
 */
export function windowGlowOffsets(
  level: number,
  salt: number,
  hw: number,
  depth: number,
): Array<{ ox: number; oy: number }> {
  if (level < 2) return [];
  const look = saltLook(salt);
  const rowY = -depth * 0.42;
  const doorY = -depth * 0.18;
  const span = Math.max(6, hw * 0.7);
  if (look.winMode === 0) {
    return [
      { ox: -span * 0.35, oy: rowY },
      { ox: span * 0.35, oy: rowY },
    ];
  }
  if (look.winMode === 1) {
    return [
      { ox: -span * 0.45, oy: rowY },
      { ox: 0, oy: rowY },
      { ox: span * 0.45, oy: rowY },
    ];
  }
  // winMode 2: fewer windows → single upper + door fallback if tiny
  return [
    { ox: 0, oy: rowY },
    { ox: 0, oy: doorY },
  ];
}

/**
 * Select night-window glow slots up to `cap`, preferring buildings nearest
 * center. Each mid+ building may contribute multiple slots (window layout).
 * PURE + deterministic.
 */
export function selectWindowGlows(
  buildings: readonly WindowGlowInput[],
  cap: number,
  cx: number,
  cy: number,
): WindowGlowSlot[] {
  if (cap <= 0) return [];
  // Rank buildings by distance first, then emit their slots in that order.
  const ranked = buildings
    .filter((b) => b.level >= 2)
    .map((b) => ({ b, d2: dist2(b.x, b.y, cx, cy) }))
    .sort((a, c) => {
      if (a.d2 !== c.d2) return a.d2 - c.d2;
      return a.b.fileId < c.b.fileId ? -1 : a.b.fileId > c.b.fileId ? 1 : 0;
    });
  const out: WindowGlowSlot[] = [];
  for (const { b } of ranked) {
    const offs = windowGlowOffsets(b.level, b.salt, b.hw, b.depth);
    for (const o of offs) {
      out.push({ fileId: b.fileId, ox: o.ox, oy: o.oy });
      if (out.length >= cap) return out;
    }
  }
  return out;
}

/**
 * Layer alpha from day-phase darkness (0 noon → 1 dusk).
 * 0 below threshold; smooth ramp above.
 * PURE.
 */
export function nightWindowLayerAlpha(darkness: number): number {
  if (!Number.isFinite(darkness) || darkness < NIGHT_WINDOW_DARKNESS_THRESHOLD) {
    return 0;
  }
  const t =
    (darkness - NIGHT_WINDOW_DARKNESS_THRESHOLD) /
    (1 - NIGHT_WINDOW_DARKNESS_THRESHOLD);
  // Soft warm glow intensity — never full-opaque.
  return Math.min(1, Math.max(0, t)) * 0.72;
}

// ---------------------------------------------------------------------------
// Pure: traffic dust path sampling
// ---------------------------------------------------------------------------

export interface TrunkSegment {
  /** Stable id for determinism (e.g. roadId). */
  id: string;
  x0: number;
  y0: number;
  x1: number;
  y1: number;
  weight: number;
}

export interface DustMotePath {
  /** Sampled world position along a trunk segment. */
  x: number;
  y: number;
  /** Segment direction unit (for slow crawl). */
  dx: number;
  dy: number;
  /** Parametric speed along the segment (per step). */
  speed: number;
  /** Phase offset 0..1 for position oscillation. */
  phase: number;
  roadId: string;
}

/**
 * Sample up to `cap` dust-mote start positions on the highest-weight trunk
 * segments. Deterministic: segments sorted by weight desc, then id; samples
 * use hash of roadId + index.
 * PURE.
 */
export function sampleTrafficDust(
  segments: readonly TrunkSegment[],
  cap: number,
): DustMotePath[] {
  if (cap <= 0 || segments.length === 0) return [];
  // Prefer top-weight trunks.
  const ranked = segments
    .filter((s) => {
      const len = Math.hypot(s.x1 - s.x0, s.y1 - s.y0);
      return len > 8 && s.weight >= 3;
    })
    .slice()
    .sort((a, b) => {
      if (b.weight !== a.weight) return b.weight - a.weight;
      return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
    });
  if (ranked.length === 0) return [];

  const out: DustMotePath[] = [];
  let i = 0;
  while (out.length < cap) {
    const seg = ranked[i % ranked.length];
    const sampleIdx = Math.floor(i / ranked.length);
    const h = hashString(`${seg.id}#dust#${sampleIdx}`);
    const t = ((h % 9973) / 9973) * 0.8 + 0.1; // keep off endpoints
    const dx = seg.x1 - seg.x0;
    const dy = seg.y1 - seg.y0;
    const len = Math.hypot(dx, dy) || 1;
    const ux = dx / len;
    const uy = dy / len;
    const phase = ((h >>> 8) % 9973) / 9973;
    const speed = 0.004 + ((h >>> 16) % 1000) / 1000 * 0.006;
    out.push({
      x: seg.x0 + dx * t,
      y: seg.y0 + dy * t,
      dx: ux,
      dy: uy,
      speed,
      phase,
      roadId: seg.id,
    });
    i++;
    // Safety: don't infinite-loop if cap is huge vs segments
    if (i > cap * ranked.length && out.length > 0) break;
  }
  return out.slice(0, cap);
}

// ---------------------------------------------------------------------------
// Pure: forum cluster placement
// ---------------------------------------------------------------------------

export interface ForumAnchorInput {
  fileId: string;
  purpose: string;
  /** True when featureSource === "commons". */
  isCommons: boolean;
  x: number;
  y: number;
}

export interface ForumClusterPlan {
  fileId: string;
  x: number;
  y: number;
  /** 2..4 figures. */
  count: number;
  /** Per-figure local offsets (screen px). */
  offsets: Array<{ ox: number; oy: number }>;
  /** Bob phase seed. */
  bobPhase: number;
}

/**
 * Select static forum clusters near market/townhall/commons. Cap + nearest
 * center. Figure count 2–4 from hash(fileId). PURE + deterministic.
 */
export function selectForumClusters(
  anchors: readonly ForumAnchorInput[],
  cap: number,
  cx: number,
  cy: number,
): ForumClusterPlan[] {
  if (cap <= 0) return [];
  const eligible = anchors.filter(
    (a) =>
      a.isCommons ||
      (FORUM_PURPOSES as readonly string[]).includes(a.purpose),
  );
  const ids = selectNearestCap(eligible, cap, cx, cy);
  const byId = new Map(eligible.map((a) => [a.fileId, a]));
  const plans: ForumClusterPlan[] = [];
  for (const id of ids) {
    const a = byId.get(id);
    if (!a) continue;
    const h = hashString(`forum:${id}`);
    const count = 2 + (h % 3); // 2..4
    const bobPhase = (h % 9973) / 9973 * 4;
    const offsets: Array<{ ox: number; oy: number }> = [];
    for (let i = 0; i < count; i++) {
      const hi = hashString(`forum:${id}:f${i}`);
      const ang = ((hi % 9973) / 9973) * Math.PI * 2;
      const r = 10 + ((hi >>> 8) % 12);
      offsets.push({
        ox: Math.cos(ang) * r + ((i % 2) * 4 - 2),
        oy: Math.sin(ang) * r * 0.55 + 6,
      });
    }
    // Cluster sits slightly in front of the building anchor.
    plans.push({
      fileId: id,
      x: a.x,
      y: a.y + 14,
      count,
      offsets,
      bobPhase,
    });
  }
  return plans;
}

// ---------------------------------------------------------------------------
// Pure: bird path (bezier sampling)
// ---------------------------------------------------------------------------

export interface BirdArc {
  /** Cubic bezier control points (screen/world). */
  p0x: number;
  p0y: number;
  p1x: number;
  p1y: number;
  p2x: number;
  p2y: number;
  p3x: number;
  p3y: number;
  /** Duration in StepClock frames. */
  frames: number;
}

/** Evaluate cubic bezier at t∈[0,1]. PURE. */
export function cubicBezier(
  t: number,
  p0: number,
  p1: number,
  p2: number,
  p3: number,
): number {
  const u = 1 - t;
  return (
    u * u * u * p0 +
    3 * u * u * t * p1 +
    3 * u * t * t * p2 +
    t * t * t * p3
  );
}

/**
 * Build a bird arc across a city AABB from a PRNG stream.
 * PURE given the rng state.
 */
export function nextBirdArc(
  rng: () => number,
  minX: number,
  minY: number,
  maxX: number,
  maxY: number,
): BirdArc {
  const pad = 40;
  const leftToRight = rng() < 0.5;
  const y0 = minY + rng() * Math.max(1, maxY - minY);
  const y1 = minY + rng() * Math.max(1, maxY - minY);
  const xA = leftToRight ? minX - pad : maxX + pad;
  const xB = leftToRight ? maxX + pad : minX - pad;
  const midX = (xA + xB) / 2;
  const lift = 30 + rng() * 80;
  return {
    p0x: xA,
    p0y: y0,
    p1x: midX - 40 + rng() * 80,
    p1y: y0 - lift,
    p2x: midX - 40 + rng() * 80,
    p2y: y1 - lift * 0.6,
    p3x: xB,
    p3y: y1,
    frames: Math.floor(90 + rng() * 90), // ~3–6s at 30fps
  };
}

// ---------------------------------------------------------------------------
// Texture bakers (once per session)
// ---------------------------------------------------------------------------

/** Soft radial puff texture (white; tinted at runtime). */
export function bakeSoftPuffTexture(renderer: AmbientTextureSource): Texture {
  const size = 32;
  const g = new Graphics();
  const cx = size / 2;
  for (let i = size / 2; i > 0; i -= 1) {
    const t = i / (size / 2);
    const a = (1 - t) * (1 - t) * 0.55;
    g.circle(cx, cx, i).fill({ color: 0xffffff, alpha: a });
  }
  const c = new Container();
  c.addChild(g);
  const tex = renderer.generateTexture({
    target: c,
    resolution: 1,
    antialias: true,
  });
  g.destroy();
  c.destroy({ children: false });
  return tex;
}

/** Tiny bird frame textures (2 flap poses, triangle-ish). */
export function bakeBirdTextures(
  renderer: AmbientTextureSource,
): [Texture, Texture] {
  const make = (open: boolean): Texture => {
    const g = new Graphics();
    // Body
    g.ellipse(0, 0, 3.2, 1.4).fill({ color: 0xffffff, alpha: 1 });
    if (open) {
      // Wings up
      g.poly([-2, 0, -7, -4, 0, -1]).fill({ color: 0xffffff, alpha: 1 });
      g.poly([2, 0, 7, -4, 0, -1]).fill({ color: 0xffffff, alpha: 1 });
    } else {
      // Wings flat/down
      g.poly([-2, 0, -7, 1, 0, 0.5]).fill({ color: 0xffffff, alpha: 1 });
      g.poly([2, 0, 7, 1, 0, 0.5]).fill({ color: 0xffffff, alpha: 1 });
    }
    const c = new Container();
    c.addChild(g);
    const tex = renderer.generateTexture({
      target: c,
      resolution: 2,
      antialias: false,
    });
    g.destroy();
    c.destroy({ children: false });
    return tex;
  };
  return [make(true), make(false)];
}

/** Soft warm radial for window glow (additive). */
export function bakeWindowGlowTexture(renderer: AmbientTextureSource): Texture {
  const size = 24;
  const g = new Graphics();
  const cx = size / 2;
  for (let i = size / 2; i > 0; i -= 1) {
    const t = i / (size / 2);
    const a = (1 - t) * (1 - t) * 0.85;
    g.circle(cx, cx, i).fill({ color: 0xffffff, alpha: a });
  }
  // Bright core
  g.circle(cx, cx, 3).fill({ color: 0xffffff, alpha: 1 });
  const c = new Container();
  c.addChild(g);
  const tex = renderer.generateTexture({
    target: c,
    resolution: 1,
    antialias: true,
  });
  g.destroy();
  c.destroy({ children: false });
  return tex;
}

/**
 * Bake 2–3 muted forum figure looks to shared textures (once per session).
 * White base is not used — figures are fully colored at bake time so rebuild
 * only reassigns Sprite.texture / position / visible.
 */
export function bakeForumFigureTextures(
  renderer: AmbientTextureSource,
): Texture[] {
  const tunics = [FORUM_TUNIC_A, FORUM_TUNIC_B, FORUM_TUNIC_C];
  const out: Texture[] = [];
  for (const tunic of tunics) {
    const g = new Graphics();
    drawCitizen(g, "citizen", {
      tunic,
      moving: false,
      phase: 0,
      actionPhase: 0,
    });
    g.alpha = 0.72;
    const c = new Container();
    c.addChild(g);
    try {
      const tex = renderer.generateTexture({
        target: c,
        resolution: 2,
        antialias: false,
      });
      out.push(tex);
    } catch {
      // headless / bake failure — skip this look
    }
    g.destroy();
    c.destroy({ children: false });
  }
  return out;
}

// ---------------------------------------------------------------------------
// Pure: per-subsystem input signatures (cheap rebuild gates)
// ---------------------------------------------------------------------------

/**
 * Chimney input sig: sorted eligible-candidate fileIds + activity flags.
 * Center included so nearest-cap ranking is stable under city extent shifts.
 * PURE.
 */
export function ambientChimneySig(
  buildings: readonly {
    fileId: string;
    purpose: string;
    salt: number;
    agentPresent?: string | null;
    lastModified: string;
  }[],
  nowMs: number,
  cx: number,
  cy: number,
): string {
  const parts: string[] = [];
  for (const b of buildings) {
    const purposeOk = (ALWAYS_CHIMNEY_PURPOSES as readonly string[]).includes(
      b.purpose,
    );
    if (!purposeOk && !saltLook(b.salt).hasChimney) continue;
    const active = isBuildingActive(
      b.agentPresent,
      parseLastModifiedMs(b.lastModified),
      nowMs,
    );
    parts.push(`${b.fileId}:${active ? 1 : 0}`);
  }
  parts.sort();
  return `${Math.round(cx)},${Math.round(cy)}|${parts.join("|")}`;
}

/**
 * Night-windows input sig: (fileId,level,salt) for level≥2 buildings + center.
 * PURE.
 */
export function ambientWindowsSig(
  buildings: readonly { fileId: string; level: number; salt: number }[],
  cx: number,
  cy: number,
): string {
  const parts: string[] = [];
  for (const b of buildings) {
    if (b.level < 2) continue;
    parts.push(`${b.fileId}:${b.level}:${b.salt}`);
  }
  parts.sort();
  return `${Math.round(cx)},${Math.round(cy)}|${parts.join("|")}`;
}

/**
 * Civic-flags input sig: sorted civic building fileIds + center.
 * PURE.
 */
export function ambientFlagsSig(
  buildings: readonly { fileId: string; purpose: string }[],
  cx: number,
  cy: number,
): string {
  const parts: string[] = [];
  for (const b of buildings) {
    if ((CIVIC_FLAG_PURPOSES as readonly string[]).includes(b.purpose)) {
      parts.push(b.fileId);
    }
  }
  parts.sort();
  return `${Math.round(cx)},${Math.round(cy)}|${parts.join("|")}`;
}

/**
 * Traffic-dust input sig: road trunk topology (id + weight + endpoints).
 * PURE. Reuses the same trunk list built for sampleTrafficDust.
 */
export function ambientDustSig(
  trunks: readonly {
    id: string;
    x0: number;
    y0: number;
    x1: number;
    y1: number;
    weight: number;
  }[],
): string {
  const parts = trunks.map(
    (t) =>
      `${t.id}:${t.weight}:${Math.round(t.x0)},${Math.round(t.y0)}>${Math.round(t.x1)},${Math.round(t.y1)}`,
  );
  parts.sort();
  return parts.join("|");
}

/**
 * Forum-clusters input sig: sorted anchor building fileIds + center.
 * PURE.
 */
export function ambientForumsSig(
  buildings: readonly {
    fileId: string;
    purpose: string;
    isCommons: boolean;
  }[],
  cx: number,
  cy: number,
): string {
  const parts: string[] = [];
  for (const b of buildings) {
    if (
      b.isCommons ||
      (FORUM_PURPOSES as readonly string[]).includes(b.purpose)
    ) {
      parts.push(b.fileId);
    }
  }
  parts.sort();
  return `${Math.round(cx)},${Math.round(cy)}|${parts.join("|")}`;
}

// ---------------------------------------------------------------------------
// PIXI: Chimney smoke system
// ---------------------------------------------------------------------------

interface SmokeParticle {
  sprite: Sprite;
  life: number;
  lifeRate: number;
  baseX: number;
  baseY: number;
  driftX: number;
  r0: number;
  active: boolean;
}

const SMOKE_POOL = 96;

export class ChimneySmokeSystem {
  readonly root = new Container();
  private particles: SmokeParticle[] = [];
  private emitters: Array<{ x: number; y: number; phase: number }> = [];
  private tex: Texture | null = null;
  private enabled = true;

  constructor() {
    this.root.eventMode = "none";
    this.root.sortableChildren = false;
  }

  bake(renderer: AmbientTextureSource): void {
    if (this.tex) return;
    try {
      this.tex = bakeSoftPuffTexture(renderer);
    } catch {
      this.tex = null;
      return;
    }
    for (let i = 0; i < SMOKE_POOL; i++) {
      const sp = new Sprite(this.tex);
      sp.anchor.set(0.5, 0.5);
      sp.tint = CHIMNEY_SMOKE_TINT;
      sp.visible = false;
      sp.alpha = 0;
      this.root.addChild(sp);
      this.particles.push({
        sprite: sp,
        life: 0,
        lifeRate: 0.4,
        baseX: 0,
        baseY: 0,
        driftX: 0,
        r0: 1,
        active: false,
      });
    }
  }

  /**
   * Rebuild emitters at world positions (iso + roof offset already applied).
   * phase from hash for desync.
   */
  setEmitters(
    emitters: Array<{ fileId: string; x: number; y: number }>,
  ): void {
    this.emitters = emitters.map((e) => ({
      x: e.x,
      y: e.y,
      phase: (hashString(e.fileId) % 9973) / 9973,
    }));
    // Park all particles
    for (const p of this.particles) {
      p.active = false;
      p.sprite.visible = false;
      p.sprite.alpha = 0;
    }
  }

  setEnabled(on: boolean): void {
    this.enabled = on;
    if (!on) {
      for (const p of this.particles) {
        p.active = false;
        p.sprite.visible = false;
        p.sprite.alpha = 0;
      }
    }
    this.root.visible = on;
  }

  /** StepClock tick — emission + particle motion. No allocation. */
  step(frame: number, halfRate: boolean): void {
    if (!this.enabled || !this.tex || this.emitters.length === 0) return;
    if (halfRate && frame % 2 !== 0) return;

    // Emit: each emitter every ~8 steps, staggered by phase.
    for (let ei = 0; ei < this.emitters.length; ei++) {
      const em = this.emitters[ei];
      const period = 8;
      const offset = Math.floor(em.phase * period);
      if ((frame + offset) % period !== 0) continue;
      // Activate up to PUFFS_PER_EMITTER dead slots
      let spawned = 0;
      for (let i = 0; i < this.particles.length && spawned < 1; i++) {
        const p = this.particles[i];
        if (p.active) continue;
        const h = hashString(`puff:${ei}:${frame}`);
        const rng = mulberry32(h);
        p.active = true;
        p.life = 0;
        p.lifeRate = 0.35 + rng() * 0.2;
        p.baseX = em.x + (rng() * 4 - 2);
        p.baseY = em.y;
        p.driftX = (rng() * 14 - 5);
        p.r0 = 0.35 + rng() * 0.25;
        p.sprite.visible = true;
        p.sprite.tint = CHIMNEY_SMOKE_TINT;
        spawned++;
      }
    }

    // Advance particles (weaker/cooler than fire smoke).
    for (const p of this.particles) {
      if (!p.active) continue;
      p.life += p.lifeRate * (halfRate ? 2 : 1) * (1 / 30);
      if (p.life >= 1) {
        p.active = false;
        p.sprite.visible = false;
        p.sprite.alpha = 0;
        continue;
      }
      const age = p.life;
      // Rise slowly, slight drift; small soft puff.
      p.sprite.x = p.baseX + p.driftX * age;
      p.sprite.y = p.baseY - age * 28;
      const sc = p.r0 + age * 0.9;
      p.sprite.scale.set(sc);
      // Max alpha ~0.28 — clearly weaker than fire smoke (~0.5).
      p.sprite.alpha = 0.28 * (1 - age);
    }
  }

  clear(): void {
    this.emitters = [];
    for (const p of this.particles) {
      p.active = false;
      p.sprite.visible = false;
      p.sprite.alpha = 0;
    }
  }

  destroy(): void {
    this.clear();
    this.root.destroy({ children: true });
    if (this.tex) {
      this.tex.destroy(true);
      this.tex = null;
    }
    this.particles = [];
  }
}

// ---------------------------------------------------------------------------
// PIXI: Birds
// ---------------------------------------------------------------------------

interface BirdSlot {
  sprite: Sprite;
  active: boolean;
  arc: BirdArc | null;
  age: number; // frames
}

export class BirdSystem {
  readonly root = new Container();
  private slots: BirdSlot[] = [];
  private frames: [Texture, Texture] | null = null;
  private rng: (() => number) | null = null;
  private bounds = { minX: -200, minY: -200, maxX: 200, maxY: 200 };
  private maxBirds = 0;
  private spawnCooldown = 0;
  private enabled = true;

  constructor() {
    this.root.eventMode = "none";
  }

  bake(renderer: AmbientTextureSource, maxBirds: number): void {
    this.maxBirds = Math.max(0, maxBirds);
    if (this.maxBirds === 0) return;
    try {
      this.frames = bakeBirdTextures(renderer);
    } catch {
      this.frames = null;
      return;
    }
    this.rng = mulberry32(BIRDS_SEED >>> 0);
    for (let i = 0; i < this.maxBirds; i++) {
      const sp = new Sprite(this.frames[0]);
      sp.anchor.set(0.5, 0.5);
      sp.tint = BIRD_TINT;
      sp.visible = false;
      sp.alpha = 0.85;
      sp.scale.set(0.9);
      this.root.addChild(sp);
      this.slots.push({ sprite: sp, active: false, arc: null, age: 0 });
    }
  }

  setBounds(minX: number, minY: number, maxX: number, maxY: number): void {
    this.bounds = { minX, minY, maxX, maxY };
  }

  setEnabled(on: boolean): void {
    this.enabled = on;
    if (!on) {
      for (const s of this.slots) {
        s.active = false;
        s.arc = null;
        s.sprite.visible = false;
      }
    }
    this.root.visible = on;
  }

  step(frame: number, halfRate: boolean): void {
    if (!this.enabled || !this.frames || !this.rng || this.maxBirds === 0) return;
    if (halfRate && frame % 2 !== 0) return;

    if (this.spawnCooldown > 0) this.spawnCooldown--;

    // Spawn into free slot
    let activeCount = 0;
    for (const s of this.slots) if (s.active) activeCount++;
    if (
      activeCount < this.maxBirds &&
      this.spawnCooldown <= 0 &&
      this.rng() < 0.04
    ) {
      for (const s of this.slots) {
        if (s.active) continue;
        s.active = true;
        s.age = 0;
        s.arc = nextBirdArc(
          this.rng,
          this.bounds.minX,
          this.bounds.minY,
          this.bounds.maxX,
          this.bounds.maxY,
        );
        s.sprite.visible = true;
        this.spawnCooldown = 45 + Math.floor(this.rng() * 60);
        break;
      }
    }

    // Advance
    for (const s of this.slots) {
      if (!s.active || !s.arc) continue;
      s.age++;
      const t = s.age / s.arc.frames;
      if (t >= 1) {
        s.active = false;
        s.arc = null;
        s.sprite.visible = false;
        continue;
      }
      s.sprite.x = cubicBezier(
        t,
        s.arc.p0x,
        s.arc.p1x,
        s.arc.p2x,
        s.arc.p3x,
      );
      s.sprite.y = cubicBezier(
        t,
        s.arc.p0y,
        s.arc.p1y,
        s.arc.p2y,
        s.arc.p3y,
      );
      // 2-frame flap
      s.sprite.texture = this.frames[(frame + Math.floor(s.age / 3)) % 2];
      // Face flight direction
      const t2 = Math.min(1, t + 0.02);
      const nx = cubicBezier(t2, s.arc.p0x, s.arc.p1x, s.arc.p2x, s.arc.p3x);
      s.sprite.scale.x = nx >= s.sprite.x ? 0.9 : -0.9;
    }
  }

  /** Number of birds currently on an arc (for tests / diagnostics). */
  get activeCount(): number {
    let n = 0;
    for (const s of this.slots) if (s.active) n++;
    return n;
  }

  clear(): void {
    for (const s of this.slots) {
      s.active = false;
      s.arc = null;
      s.sprite.visible = false;
    }
    this.spawnCooldown = 0;
    // Reseed so full clear (clearScene) is deterministic from fixed constant.
    this.rng = mulberry32(BIRDS_SEED >>> 0);
  }

  destroy(): void {
    this.clear();
    this.root.destroy({ children: true });
    if (this.frames) {
      this.frames[0].destroy(true);
      this.frames[1].destroy(true);
      this.frames = null;
    }
    this.slots = [];
  }
}

// ---------------------------------------------------------------------------
// PIXI: Night windows (pooled to profile cap in bake)
// ---------------------------------------------------------------------------

export class NightWindowsSystem {
  readonly root = new Container();
  /** Pre-allocated sprite pool (fixed after bake). */
  private pool: Sprite[] = [];
  private tex: Texture | null = null;
  private slots: WindowGlowSlot[] = [];
  private buildingPos = new Map<string, { x: number; y: number }>();
  private enabled = true;
  private baked = false;

  constructor() {
    this.root.eventMode = "none";
    // Additive warm glow
    this.root.blendMode = "add";
  }

  /** Pre-allocate sprites to profile cap once. rebuild only reassigns. */
  bake(renderer: AmbientTextureSource, cap: number): void {
    if (this.baked) return;
    this.baked = true;
    try {
      this.tex = bakeWindowGlowTexture(renderer);
    } catch {
      this.tex = null;
      return;
    }
    const n = Math.max(0, cap);
    for (let i = 0; i < n; i++) {
      const sp = new Sprite(this.tex);
      sp.anchor.set(0.5, 0.5);
      sp.tint = WINDOW_GLOW_TINT;
      sp.scale.set(0.55);
      sp.alpha = 1;
      sp.visible = false;
      this.root.addChild(sp);
      this.pool.push(sp);
    }
  }

  /**
   * Reassign pooled sprites to planned slots. Parks excess with visible=false.
   * No Sprite allocation after bake.
   */
  rebuild(
    slots: readonly WindowGlowSlot[],
    positions: Map<string, { x: number; y: number }>,
  ): void {
    this.slots = slots.slice();
    this.buildingPos = positions;
    if (!this.tex || this.pool.length === 0) {
      // Still park everything if pool empty.
      for (const sp of this.pool) sp.visible = false;
      this.root.alpha = 0;
      return;
    }
    let i = 0;
    for (const slot of this.slots) {
      if (i >= this.pool.length) break;
      const pos = positions.get(slot.fileId);
      if (!pos) continue;
      const sp = this.pool[i];
      sp.position.set(pos.x + slot.ox, pos.y + slot.oy);
      sp.tint = WINDOW_GLOW_TINT;
      sp.visible = true;
      i++;
    }
    for (; i < this.pool.length; i++) {
      this.pool[i].visible = false;
    }
    // Preserve layer alpha; step() drives it from darkness.
  }

  /** Pool size after bake (for tests). */
  get poolSize(): number {
    return this.pool.length;
  }

  /** Active (visible) sprite count (for tests). */
  get activeCount(): number {
    let n = 0;
    for (const sp of this.pool) if (sp.visible) n++;
    return n;
  }

  /** Pooled sprite instances (identity stable across rebuild; for tests). */
  get poolSprites(): readonly Sprite[] {
    return this.pool;
  }

  setEnabled(on: boolean): void {
    this.enabled = on;
    if (!on) this.root.alpha = 0;
    this.root.visible = on;
  }

  /** Only mutates root.alpha from darkness (dayPhase). */
  step(darkness: number): void {
    if (!this.enabled) {
      this.root.alpha = 0;
      return;
    }
    this.root.alpha = nightWindowLayerAlpha(darkness);
  }

  clear(): void {
    for (const sp of this.pool) sp.visible = false;
    this.slots = [];
    this.buildingPos.clear();
    this.root.alpha = 0;
  }

  destroy(): void {
    this.clear();
    this.root.destroy({ children: true });
    if (this.tex) {
      this.tex.destroy(true);
      this.tex = null;
    }
    this.pool = [];
    this.baked = false;
  }
}

// ---------------------------------------------------------------------------
// PIXI: Traffic dust (pooled to profile cap in bake)
// ---------------------------------------------------------------------------

interface DustSlot {
  sprite: Sprite;
  path: DustMotePath | null;
  t: number;
  active: boolean;
}

export class TrafficDustSystem {
  readonly root = new Container();
  private slots: DustSlot[] = [];
  private tex: Texture | null = null;
  private enabled = true;
  private farZoom = true;
  private baked = false;

  constructor() {
    this.root.eventMode = "none";
  }

  /** Pre-allocate sprites to profile cap once. rebuild only reassigns. */
  bake(renderer: AmbientTextureSource, cap: number): void {
    if (this.baked) return;
    this.baked = true;
    try {
      this.tex = bakeSoftPuffTexture(renderer);
    } catch {
      this.tex = null;
      return;
    }
    const n = Math.max(0, cap);
    for (let i = 0; i < n; i++) {
      const sp = new Sprite(this.tex);
      sp.anchor.set(0.5, 0.5);
      sp.tint = DUST_TINT;
      sp.scale.set(0.18);
      sp.alpha = 0.35;
      sp.visible = false;
      this.root.addChild(sp);
      this.slots.push({
        sprite: sp,
        path: null,
        t: 0,
        active: false,
      });
    }
  }

  /**
   * Reassign pooled slots to paths. Parks excess with visible=false.
   * No Sprite allocation after bake.
   */
  rebuild(paths: readonly DustMotePath[]): void {
    if (!this.tex) {
      for (const s of this.slots) {
        s.active = false;
        s.path = null;
        s.sprite.visible = false;
      }
      return;
    }
    let i = 0;
    for (; i < paths.length && i < this.slots.length; i++) {
      const path = paths[i];
      const s = this.slots[i];
      s.path = path;
      s.t = path.phase;
      s.active = true;
      s.sprite.tint = DUST_TINT;
      s.sprite.position.set(path.x, path.y);
      s.sprite.visible = true;
    }
    for (; i < this.slots.length; i++) {
      const s = this.slots[i];
      s.active = false;
      s.path = null;
      s.sprite.visible = false;
    }
  }

  /** Pool size after bake (for tests). */
  get poolSize(): number {
    return this.slots.length;
  }

  get activeCount(): number {
    let n = 0;
    for (const s of this.slots) if (s.active) n++;
    return n;
  }

  get poolSprites(): readonly Sprite[] {
    return this.slots.map((s) => s.sprite);
  }

  setEnabled(on: boolean): void {
    this.enabled = on;
    this.root.visible = on && this.farZoom;
  }

  /** Visible only when zoomed out past lodAgents (walkers hidden). */
  setFarZoom(far: boolean): void {
    this.farZoom = far;
    this.root.visible = this.enabled && far;
  }

  step(frame: number, halfRate: boolean): void {
    if (!this.enabled || !this.farZoom) return;
    if (halfRate && frame % 2 !== 0) return;
    for (const s of this.slots) {
      if (!s.active || !s.path) continue;
      s.t = (s.t + s.path.speed) % 1;
      // Slow crawl along direction with tiny lateral wobble.
      const travel = (s.t - 0.5) * 40;
      const wobble = Math.sin((frame + s.path.phase * 100) * 0.08) * 1.2;
      s.sprite.x = s.path.x + s.path.dx * travel + s.path.dy * wobble;
      s.sprite.y = s.path.y + s.path.dy * travel - s.path.dx * wobble;
      s.sprite.alpha = 0.22 + 0.12 * Math.sin(frame * 0.05 + s.path.phase * 6);
    }
  }

  clear(): void {
    for (const s of this.slots) {
      s.active = false;
      s.path = null;
      s.sprite.visible = false;
    }
  }

  destroy(): void {
    this.clear();
    this.root.destroy({ children: true });
    if (this.tex) {
      this.tex.destroy(true);
      this.tex = null;
    }
    this.slots = [];
    this.baked = false;
  }
}

// ---------------------------------------------------------------------------
// PIXI: Forum clusters (pooled containers + shared figure textures)
// ---------------------------------------------------------------------------

interface ForumFigureSlot {
  sprite: Sprite;
  baseOx: number;
  baseOy: number;
}

interface ForumClusterRuntime {
  container: Container;
  figures: ForumFigureSlot[];
  plan: ForumClusterPlan | null;
  active: boolean;
}

export class ForumClusterSystem {
  readonly root = new Container();
  private clusters: ForumClusterRuntime[] = [];
  private figureTex: Texture[] = [];
  private enabled = true;
  private baked = false;

  constructor() {
    this.root.eventMode = "none";
  }

  /**
   * Bake 2–3 muted figure textures once; pre-allocate cluster containers and
   * figure Sprites to profile cap. rebuild only reassigns.
   */
  bake(renderer: AmbientTextureSource, maxClusters: number): void {
    if (this.baked) return;
    this.baked = true;
    try {
      this.figureTex = bakeForumFigureTextures(renderer);
    } catch {
      this.figureTex = [];
    }
    if (this.figureTex.length === 0) return;
    const n = Math.max(0, maxClusters);
    for (let c = 0; c < n; c++) {
      const container = new Container();
      container.visible = false;
      container.eventMode = "none";
      const figures: ForumFigureSlot[] = [];
      for (let f = 0; f < FORUM_MAX_FIGURES; f++) {
        const sp = new Sprite(this.figureTex[f % this.figureTex.length]);
        sp.anchor.set(0.5, 1);
        sp.scale.set(0.38);
        sp.alpha = 1; // alpha baked into texture via draw alpha
        sp.visible = false;
        container.addChild(sp);
        figures.push({ sprite: sp, baseOx: 0, baseOy: 0 });
      }
      this.root.addChild(container);
      this.clusters.push({
        container,
        figures,
        plan: null,
        active: false,
      });
    }
  }

  /**
   * Reassign pooled clusters to plans. Parks excess with visible=false.
   * No Graphics/Container allocation after bake.
   */
  rebuild(plans: readonly ForumClusterPlan[]): void {
    if (this.figureTex.length === 0 || this.clusters.length === 0) return;
    for (let i = 0; i < this.clusters.length; i++) {
      const c = this.clusters[i];
      if (i >= plans.length) {
        c.active = false;
        c.plan = null;
        c.container.visible = false;
        for (const f of c.figures) f.sprite.visible = false;
        continue;
      }
      const plan = plans[i];
      c.plan = plan;
      c.active = true;
      c.container.visible = true;
      c.container.position.set(plan.x, plan.y);
      for (let f = 0; f < c.figures.length; f++) {
        const fig = c.figures[f];
        if (f >= plan.count) {
          fig.sprite.visible = false;
          continue;
        }
        fig.sprite.texture = this.figureTex[f % this.figureTex.length];
        fig.baseOx = plan.offsets[f].ox;
        fig.baseOy = plan.offsets[f].oy;
        fig.sprite.position.set(fig.baseOx, fig.baseOy);
        fig.sprite.visible = true;
      }
    }
  }

  get poolSize(): number {
    return this.clusters.length;
  }

  get activeCount(): number {
    let n = 0;
    for (const c of this.clusters) if (c.active) n++;
    return n;
  }

  /** First sprite of each cluster container (identity stable; for tests). */
  get poolSprites(): readonly Sprite[] {
    const out: Sprite[] = [];
    for (const c of this.clusters) {
      for (const f of c.figures) out.push(f.sprite);
    }
    return out;
  }

  setEnabled(on: boolean): void {
    this.enabled = on;
    this.root.visible = on;
  }

  /** Tiny deterministic bob — no redraw, only y offset. */
  step(frame: number, halfRate: boolean): void {
    if (!this.enabled) return;
    if (halfRate && frame % 2 !== 0) return;
    for (const c of this.clusters) {
      if (!c.active || !c.plan) continue;
      for (let i = 0; i < c.figures.length; i++) {
        if (!c.figures[i].sprite.visible) continue;
        const phase = c.plan.bobPhase + i * 1.7;
        const bob = Math.sin(frame * 0.12 + phase) > 0 ? -1 : 0;
        c.figures[i].sprite.y = c.figures[i].baseOy + bob;
      }
    }
  }

  clear(): void {
    for (const c of this.clusters) {
      c.active = false;
      c.plan = null;
      c.container.visible = false;
      for (const f of c.figures) f.sprite.visible = false;
    }
  }

  destroy(): void {
    this.clear();
    this.root.destroy({ children: true });
    for (const t of this.figureTex) t.destroy(true);
    this.figureTex = [];
    this.clusters = [];
    this.baked = false;
  }
}

// ---------------------------------------------------------------------------
// Civic flags helper (mutates kit Flag.t — no new cloth sim)
// ---------------------------------------------------------------------------

/**
 * Marker on Flag instances created by ambient life (not kit-native).
 * Kit flags from building builders must never receive this mark.
 */
export type AmbientMarkedFlag = Flag & { fromAmbientLife?: true };

/** True when this Flag was created by applyCivicFlagPhases (ambient life). */
export function isAmbientLifeFlag(a: AnimInstance): a is AmbientMarkedFlag {
  return a instanceof Flag && (a as AmbientMarkedFlag).fromAmbientLife === true;
}

/**
 * Apply deterministic wind phase to Flag anims on selected civic buildings.
 * Creates a small Flag when the building has none (temple/theater/library),
 * marking it `fromAmbientLife` so deselection can remove it without touching
 * kit-native flags. Returns phased count + newly created fileIds.
 * Mutates node.kitAnims / container.
 */
export function applyCivicFlagPhases(
  targets: ReadonlyArray<{
    fileId: string;
    kitAnims: AnimInstance[];
    container: Container;
    depth: number;
  }>,
): { count: number; createdIds: string[] } {
  let n = 0;
  const createdIds: string[] = [];
  for (const t of targets) {
    const phase = flagPhaseOffset(t.fileId);
    let flag: AmbientMarkedFlag | null = null;
    for (const a of t.kitAnims) {
      if (a.kind === "flag" && a instanceof Flag) {
        flag = a as AmbientMarkedFlag;
        break;
      }
    }
    if (!flag) {
      // Small pennant on the ridge — kit Flag class, ambient-owned instance.
      flag = new Flag(
        0,
        -Math.max(18, t.depth * 0.85),
        0.75,
        MAT.red,
      ) as AmbientMarkedFlag;
      flag.fromAmbientLife = true;
      t.container.addChild(flag.node);
      t.kitAnims.push(flag);
      createdIds.push(t.fileId);
    }
    flag.t = phase;
    n++;
  }
  return { count: n, createdIds };
}

/**
 * Remove ambient-added Flag anims from buildings no longer in the civic
 * selection. Kit-native flags (no `fromAmbientLife` mark) are untouched.
 * Mutates kitAnims + destroys flag nodes. PURE selection inputs only.
 */
export function removeAmbientCivicFlags(
  buildings: ReadonlyArray<{
    fileId: string;
    kitAnims: AnimInstance[];
  }>,
  keepIds: ReadonlySet<string>,
  ambientFlagIds: Set<string>,
): void {
  if (ambientFlagIds.size === 0) return;
  const byId = new Map(buildings.map((b) => [b.fileId, b]));
  // Drop tracking for buildings that no longer exist.
  for (const id of [...ambientFlagIds]) {
    if (!byId.has(id)) ambientFlagIds.delete(id);
  }
  for (const id of [...ambientFlagIds]) {
    if (keepIds.has(id)) continue;
    const b = byId.get(id);
    if (b) {
      for (let i = b.kitAnims.length - 1; i >= 0; i--) {
        const a = b.kitAnims[i];
        if (!isAmbientLifeFlag(a)) continue;
        a.node.removeFromParent();
        a.node.destroy({ children: true });
        b.kitAnims.splice(i, 1);
      }
    }
    ambientFlagIds.delete(id);
  }
}

/** Map AmbientLifeBuildingView → removeAmbientCivicFlags shape. */
function viewsAsFlagTargets(
  buildings: readonly AmbientLifeBuildingView[],
): Array<{ fileId: string; kitAnims: AnimInstance[] }> {
  return buildings.map((b) => ({
    fileId: b.building.fileId,
    kitAnims: b.kitAnims,
  }));
}

// ---------------------------------------------------------------------------
// AmbientLifeManager — owns all systems, wired by PolisRenderer
// ---------------------------------------------------------------------------

export interface AmbientLifeBuildingView {
  building: Building;
  iso: { x: number; y: number };
  salt: number;
  level: number;
  depth: number;
  hw: number;
  kitAnims: AnimInstance[];
  container: Container;
}

export interface AmbientLifeTrunkSeg {
  id: string;
  x0: number;
  y0: number;
  x1: number;
  y1: number;
  weight: number;
}

export class AmbientLifeManager {
  readonly root = new Container();
  readonly chimney = new ChimneySmokeSystem();
  readonly birds = new BirdSystem();
  readonly windows = new NightWindowsSystem();
  readonly dust = new TrafficDustSystem();
  readonly forums = new ForumClusterSystem();

  private profile: RenderProfile;
  private baked = false;
  /** Last applied per-subsystem input signatures (rebuild gates). */
  private sigs = {
    chimney: "",
    windows: "",
    flags: "",
    dust: "",
    forums: "",
  };
  /** fileIds whose Flag was created by ambient life (not kit-native). */
  private ambientFlagIds = new Set<string>();

  constructor(profile: RenderProfile) {
    this.profile = profile;
    this.root.eventMode = "none";
    this.root.addChild(this.chimney.root);
    this.root.addChild(this.windows.root);
    this.root.addChild(this.forums.root);
    this.root.addChild(this.dust.root);
    this.root.addChild(this.birds.root);
  }

  bake(renderer: AmbientTextureSource): void {
    if (this.baked) return;
    const p = this.profile;
    this.chimney.bake(renderer);
    this.birds.bake(renderer, p.maxBirds);
    this.windows.bake(renderer, p.maxNightWindows);
    this.dust.bake(renderer, p.maxTrafficDust);
    this.forums.bake(renderer, p.maxForumClusters);
    this.baked = true;
  }

  /**
   * Rebuild from city data with per-subsystem input-signature gating.
   * Each system rebuilds ONLY when its signature changed. Birds: bounds only
   * (never cleared here — F12; clearScene uses clear()).
   */
  rebuild(opts: {
    buildings: readonly AmbientLifeBuildingView[];
    trunks: readonly AmbientLifeTrunkSeg[];
    centerX: number;
    centerY: number;
    nowMs: number;
  }): void {
    const { buildings, trunks, centerX, centerY, nowMs } = opts;
    const p = this.profile;

    // Birds: update bounds only — never clear/reseed on live diffs (F12).
    let minX = Infinity,
      minY = Infinity,
      maxX = -Infinity,
      maxY = -Infinity;
    for (const b of buildings) {
      minX = Math.min(minX, b.iso.x);
      minY = Math.min(minY, b.iso.y);
      maxX = Math.max(maxX, b.iso.x);
      maxY = Math.max(maxY, b.iso.y);
    }
    if (!Number.isFinite(minX)) {
      minX = -100;
      minY = -100;
      maxX = 100;
      maxY = 100;
    }
    this.birds.setBounds(minX, minY, maxX, maxY);

    // --- Chimney ---
    const chimneyInputs: ChimneyEmitterInput[] = buildings.map((b) => ({
      fileId: b.building.fileId,
      purpose: b.building.purpose,
      salt: b.salt,
      agentPresent: b.building.agentPresent,
      lastModified: b.building.lastModified,
      x: b.iso.x,
      y: b.iso.y,
    }));
    const chimneySig = ambientChimneySig(
      chimneyInputs,
      nowMs,
      centerX,
      centerY,
    );
    if (chimneySig !== this.sigs.chimney) {
      this.sigs.chimney = chimneySig;
      const chimneyIds = selectChimneyEmitters(
        chimneyInputs,
        p.maxChimneySmoke,
        centerX,
        centerY,
        nowMs,
      );
      const chimneySet = new Set(chimneyIds);
      const emitters = buildings
        .filter((b) => chimneySet.has(b.building.fileId))
        .map((b) => ({
          fileId: b.building.fileId,
          x: b.iso.x,
          // Roof-ish: slightly above iso anchor
          y: b.iso.y - Math.max(12, b.depth * 0.55),
        }));
      this.chimney.setEmitters(emitters);
    }

    // --- Civic flags ---
    const flagInputs = buildings.map((b) => ({
      fileId: b.building.fileId,
      purpose: b.building.purpose,
      x: b.iso.x,
      y: b.iso.y,
    }));
    const flagsSig = ambientFlagsSig(flagInputs, centerX, centerY);
    if (flagsSig !== this.sigs.flags) {
      this.sigs.flags = flagsSig;
      const flagIds = selectCivicFlags(
        flagInputs,
        p.maxCivicFlags,
        centerX,
        centerY,
      );
      const flagSet = new Set(flagIds);
      // F9: drop ambient flags on buildings that fell out of selection.
      removeAmbientCivicFlags(
        viewsAsFlagTargets(buildings),
        flagSet,
        this.ambientFlagIds,
      );
      const applied = applyCivicFlagPhases(
        buildings
          .filter((b) => flagSet.has(b.building.fileId))
          .map((b) => ({
            fileId: b.building.fileId,
            kitAnims: b.kitAnims,
            container: b.container,
            depth: b.depth,
          })),
      );
      for (const id of applied.createdIds) this.ambientFlagIds.add(id);
    }

    // --- Night windows ---
    const windowInputs = buildings.map((b) => ({
      fileId: b.building.fileId,
      level: b.level,
      salt: b.salt,
      x: b.iso.x,
      y: b.iso.y,
      depth: b.depth,
      hw: b.hw,
    }));
    const windowsSig = ambientWindowsSig(windowInputs, centerX, centerY);
    if (windowsSig !== this.sigs.windows) {
      this.sigs.windows = windowsSig;
      const glowSlots = selectWindowGlows(
        windowInputs,
        p.maxNightWindows,
        centerX,
        centerY,
      );
      const posMap = new Map(
        buildings.map((b) => [b.building.fileId, { x: b.iso.x, y: b.iso.y }]),
      );
      this.windows.rebuild(glowSlots, posMap);
    }

    // --- Traffic dust ---
    const dustSig = ambientDustSig(trunks);
    if (dustSig !== this.sigs.dust) {
      this.sigs.dust = dustSig;
      const dustPaths = sampleTrafficDust(trunks, p.maxTrafficDust);
      this.dust.rebuild(dustPaths);
    }

    // --- Forum clusters ---
    const forumInputs = buildings.map((b) => ({
      fileId: b.building.fileId,
      purpose: b.building.purpose,
      isCommons: b.building.featureSource === "commons",
      x: b.iso.x,
      y: b.iso.y,
    }));
    const forumsSig = ambientForumsSig(forumInputs, centerX, centerY);
    if (forumsSig !== this.sigs.forums) {
      this.sigs.forums = forumsSig;
      const forumPlans = selectForumClusters(
        forumInputs,
        p.maxForumClusters,
        centerX,
        centerY,
      );
      this.forums.rebuild(forumPlans);
    }
  }

  /** Last applied input signatures (for tests). */
  get inputSignatures(): Readonly<{
    chimney: string;
    windows: string;
    flags: string;
    dust: string;
    forums: string;
  }> {
    return this.sigs;
  }

  /** Ambient-added civic flag fileIds (for tests). */
  get ambientAddedFlagIds(): ReadonlySet<string> {
    return this.ambientFlagIds;
  }

  /**
   * Per StepClock tick. Budget rung gates systems; dayPhase drives windows.
   * zoomScale + lodAgents gate traffic dust (far only).
   */
  step(opts: {
    frame: number;
    rung: BudgetRung;
    dayPhase: number;
    zoomScale: number;
    lodAgents: number;
  }): void {
    const gates = ambientLifeGates(opts.rung);
    const half = gates.halfRate;

    this.chimney.setEnabled(gates.chimneySmoke);
    this.birds.setEnabled(gates.birds);
    this.windows.setEnabled(gates.nightWindows);
    this.dust.setEnabled(gates.trafficDust);
    this.forums.setEnabled(gates.forumBob);

    if (gates.chimneySmoke) this.chimney.step(opts.frame, half);
    if (gates.birds) this.birds.step(opts.frame, half);
    if (gates.nightWindows) this.windows.step(opts.dayPhase);
    // Dust only when walkers are hidden (scale < lodAgents).
    this.dust.setFarZoom(opts.zoomScale < opts.lodAgents);
    if (gates.trafficDust) this.dust.step(opts.frame, half);
    if (gates.forumBob) this.forums.step(opts.frame, half);
  }

  clear(): void {
    this.chimney.clear();
    this.birds.clear();
    this.windows.clear();
    this.dust.clear();
    this.forums.clear();
    this.sigs = {
      chimney: "",
      windows: "",
      flags: "",
      dust: "",
      forums: "",
    };
    this.ambientFlagIds.clear();
  }

  destroy(): void {
    this.chimney.destroy();
    this.birds.destroy();
    this.windows.destroy();
    this.dust.destroy();
    this.forums.destroy();
    this.ambientFlagIds.clear();
    // Detach before destroy so PolisRenderer's layers.effects.destroy({children})
    // does not double-free this container.
    this.root.removeFromParent();
    this.root.destroy({ children: false });
  }
}

// Re-export tierRank for callers that map visualTier → level.
export { tierRank };
