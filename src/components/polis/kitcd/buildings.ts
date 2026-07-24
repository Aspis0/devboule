/* =========================================================================
   buildings.ts — Procedural Greek buildings (faithful port of buildings_a.js
   + buildings_b.js, PixiJS v7 → v8 / TS).

   temple · house · fortress · tower · lighthouse · market · warehouse ·
   workshop · conduit · baths · theater · harbor · library · townhall · unknown

   Each builder: fn(level 0..4, opt) -> { container, body, anims, foot:[W,D] }.

   PORTED 1:1: every builder's geometry, props, levels, animated parts EXACTLY
   as the source. `ISO`/`ANIM`/`PROP` references became imports from the sibling
   kitcd modules.

   DETERMINISM: the source builders contain NO direct Math.random — all static
   placement is fixed numeric geometry, and the only randomness lives inside the
   ANIM classes (time-based flicker/wave phase) and detail.ts's seeded sin-hash.
   So the city is already reproducible byte-for-byte; nothing here needed an
   rng seed. (Animation randomness is intentionally left alone.)
   ========================================================================= */

import { Container, Graphics, Text } from "pixi.js";
import * as ISO from "./iso";
import { MAT } from "./iso";
import { Flame, Beacon, Flag, Smoke, Water, type AnimInstance } from "./anims";
import { PROP } from "./detail";

const M = MAT;

export interface BuiltResult {
  container: Container;
  body: Graphics;
  anims: AnimInstance[];
  foot: [number, number];
}

export interface BuilderOpt {
  outline?: boolean;
  /**
   * Procedural salt 0..N-1 for cosmetic variants (roof/wall tint, chimney,
   * window arrangement, small accents). Footprint and silhouette stay fixed.
   * Default 0 (canonical look).
   */
  salt?: number;
}

export type Builder = (L: number, opt: BuilderOpt) => BuiltResult;

/**
 * Bounded, tasteful per-salt look derived only from existing kit materials
 * (shade/mix — no new raw hex). Deterministic for a given salt.
 */
export interface SaltLook {
  /** Roof colour (terracotta ± small palette blend). */
  roof: number;
  /** Multiplier applied via ISO.shade to wall materials (~0.96–1.04). */
  wallF: number;
  /** Door / wood panel shade factor. */
  door: number;
  /** Whether to emit chimney smoke (and any chimney stack geometry). */
  hasChimney: boolean;
  /**
   * Shared lateral bias in grid units (−|0|+) applied to chimney stack/smoke
   * and to workshop door placement (same wall-face drift).
   */
  lateralDx: number;
  /** Window layout mode: 0 default, 1 wider spacing, 2 fewer windows. */
  winMode: number;
  /** Small awning / pergola accent strip. */
  hasAwning: boolean;
  /** Awning / accent cloth colour from the kit palette. */
  accent: number;
}

/** Resolve cosmetic picks for a salt. PURE + deterministic. */
export function saltLook(salt = 0): SaltLook {
  const s = ((salt % 4) + 4) % 4;
  const roofBlends = [
    M.terracotta,
    ISO.mix(M.terracotta, M.ochre, 0.22),
    ISO.mix(M.terracotta, M.red, 0.16),
    ISO.mix(M.terracotta, M.plinth, 0.14),
  ];
  const roofF = [1.0, 0.94, 1.05, 0.97][s];
  const accents = [M.blue, M.red, M.ochre, M.blueDeep];
  return {
    roof: ISO.shade(roofBlends[s], roofF),
    wallF: [1.0, 1.035, 0.965, 1.02][s],
    door: ISO.shade(M.wood, [0.6, 0.52, 0.68, 0.55][s]),
    hasChimney: s !== 1,
    lateralDx: [-0.08, 0, 0.1, -0.04][s],
    winMode: s % 3,
    hasAwning: s === 2 || s === 3,
    accent: accents[s],
  };
}

/** Shade a wall material by the salt wall factor. */
function wallCol(base: number, look: SaltLook): number {
  return ISO.shade(base, look.wallF);
}

/**
 * Small masonry chimney stack — house-scale, tasteful:
 *   1. lower body rect (stone)
 *   2. slightly narrower upper stack
 *   3. wider cap slab (stone/terracotta blend)
 *   4. dark flue mouth lip
 *
 * `gx,gy` is the plan centre of the stack; `baseZ` should sit flush on the roof
 * slope near the ridge (caller-chosen). `lateralDx` shifts along +gx.
 * Returns the world-Z of the flue lip (for smoke placement).
 * Materials from kit only — no raw hex.
 */
function drawChimneyStack(
  bx: Setup["bx"],
  gx: number,
  gy: number,
  baseZ: number,
  scale = 1,
  lateralDx = 0,
): number {
  const s = Math.max(0.55, Math.min(1.15, scale));
  // Lower body
  const bodyW = 0.13 * s;
  const bodyD = 0.13 * s;
  const bodyH = 0.15 * s;
  // Upper stack (slightly narrower)
  const neckW = 0.105 * s;
  const neckD = 0.105 * s;
  const neckH = 0.11 * s;
  // Cap slab — slightly wider than body
  const capW = 0.17 * s;
  const capD = 0.17 * s;
  const capH = 0.045 * s;
  // Dark flue mouth
  const flueW = 0.055 * s;
  const flueD = 0.055 * s;
  const flueH = 0.016 * s;

  const ox = gx + lateralDx - bodyW / 2;
  const oy = gy - bodyD / 2;

  const body = ISO.shade(M.stone, 0.92);
  const neck = ISO.shade(M.stone, 0.84);
  const cap = ISO.mix(M.stone, M.terraDark, 0.28);
  const flue = ISO.shade(M.plinthDk, 0.5);

  bx(ox, oy, baseZ, bodyW, bodyD, bodyH, body);
  bx(
    ox + (bodyW - neckW) / 2,
    oy + (bodyD - neckD) / 2,
    baseZ + bodyH,
    neckW,
    neckD,
    neckH,
    neck,
  );
  bx(
    ox + (bodyW - capW) / 2,
    oy + (bodyD - capD) / 2,
    baseZ + bodyH + neckH,
    capW,
    capD,
    capH,
    cap,
  );
  bx(
    ox + (bodyW - flueW) / 2,
    oy + (bodyD - flueD) / 2,
    baseZ + bodyH + neckH + capH,
    flueW,
    flueD,
    flueH,
    flue,
  );

  return baseZ + bodyH + neckH + capH + flueH;
}

// shared registry (display + accent) --------------------------------------
export const BUILD_META = {
  order: [
    "temple",
    "house",
    "fortress",
    "tower",
    "lighthouse",
    "market",
    "warehouse",
    "workshop",
    "conduit",
    "baths",
    "theater",
    "harbor",
    "library",
    "townhall",
    "unknown",
  ],
  info: {
    temple: { name: "Naos", cat: "sacro", accent: M.blue },
    house: { name: "Oikos", cat: "abitazione", accent: M.terracotta },
    fortress: { name: "Phrourion", cat: "militare", accent: M.red },
    tower: { name: "Pyrgos", cat: "militare", accent: M.red },
    lighthouse: { name: "Pharos", cat: "porto", accent: M.gold },
    market: { name: "Agora", cat: "civile", accent: M.red },
    warehouse: { name: "Apotheke", cat: "civile", accent: M.wood },
    workshop: { name: "Ergasterion", cat: "produzione", accent: M.ochre },
    conduit: { name: "Hydragogeion", cat: "infrastruttura", accent: M.blue },
    baths: { name: "Balaneion", cat: "civile", accent: M.water },
    theater: { name: "Theatron", cat: "cultura", accent: M.red },
    harbor: { name: "Limen", cat: "porto", accent: M.blue },
    library: { name: "Bibliotheke", cat: "cultura", accent: M.blue },
    townhall: { name: "Bouleuterion", cat: "civile", accent: M.gold },
    unknown: { name: "Agnoston", cat: "fallback", accent: 0x8a8478 },
  },
} as const;

interface Setup {
  proj: ISO.Proj;
  c: Container;
  g: Graphics;
  out: boolean;
  bx: (
    x: number,
    y: number,
    z: number,
    w: number,
    d: number,
    h: number,
    col: number,
    o?: ISO.BoxOpt,
  ) => ISO.BoxResult;
  anims: AnimInstance[];
}

function setup(W: number, D: number, opt?: BuilderOpt): Setup {
  const proj = ISO.makeProj(W, D);
  const c = new Container();
  const g = new Graphics();
  c.addChild(g);
  const out = !!(opt && opt.outline);
  const TEX: Record<number, string> = {};
  TEX[M.marble] = "marble";
  TEX[M.marbleCool] = "marble";
  TEX[M.marbleWarm] = "plaster";
  TEX[M.plaster] = "plaster";
  TEX[M.mud] = "plaster";
  TEX[M.stone] = "ashlar";
  TEX[M.plinth] = "ashlar";
  TEX[M.plinthDk] = "ashlar";
  TEX[M.wood] = "wood";
  TEX[M.woodLight] = "wood";
  const bx = (
    x: number,
    y: number,
    z: number,
    w: number,
    d: number,
    h: number,
    col: number,
    o?: ISO.BoxOpt,
  ): ISO.BoxResult => {
    o = o || {};
    const t = o.tex !== undefined ? o.tex : TEX[col];
    return ISO.box(
      g,
      proj,
      x,
      y,
      z,
      w,
      d,
      h,
      col,
      Object.assign({ outline: out }, o, { tex: t }),
    );
  };
  return { proj, c, g, out, bx, anims: [] };
}

// ====================== TEMPLE ==========================================
const temple: Builder = function (L, opt) {
  const sizes = [
    [2, 3],
    [2, 3],
    [3, 4],
    [3, 5],
    [4, 6],
  ][L];
  const [W, D] = sizes;
  const s = setup(W, D, opt);
  const { proj, g, c, out, bx, anims } = s;

  const nStep = [2, 2, 3, 3, 4][L];
  const stepH = 0.13;
  const inset = 0.16;
  const topZ = ISO.steps(g, proj, 0, 0, 0, W, D, nStep, stepH, inset, M.stone);
  const ins = inset * nStep;
  const ix = ins;
  const iy = ins;
  const iw = W - 2 * ins;
  const id = D - 2 * ins;
  const colH = 1.45 + L * 0.16;
  const colR = 0.11 + L * 0.004;
  const peripteral = L >= 2;
  const frontN = [2, 3, 4, 4, 6][L];
  const sideN = [3, 3, 5, 6, 8][L];

  // back colonnade
  if (peripteral)
    ISO.colonnade(g, proj, ix, iy, ix + iw, iy, topZ, colH, colR, frontN, M.marble, {
      ionic: false,
    });
  // cella
  const cw = iw * 0.6;
  const cd = id * 0.76;
  const cx0 = ix + (iw - cw) / 2;
  const cy0 = iy + (id - cd) / 2;
  bx(cx0, cy0, topZ, cw, cd, colH * 0.94, M.marbleWarm);
  ISO.panelLeft(
    g,
    proj,
    cx0 + cw * 0.36,
    cy0 + cd,
    topZ,
    cw * 0.28,
    colH * 0.6,
    ISO.shade(M.wood, 0.7),
  ); // door
  // side colonnades
  if (peripteral) {
    ISO.colonnade(g, proj, ix, iy, ix, iy + id, topZ, colH, colR, sideN, M.marble, {});
    ISO.colonnade(
      g,
      proj,
      ix + iw,
      iy,
      ix + iw,
      iy + id,
      topZ,
      colH,
      colR,
      sideN,
      M.marble,
      {},
    );
  }
  // entablature + frieze
  const ez = topZ + colH;
  bx(ix - 0.06, iy - 0.06, ez, iw + 0.12, id + 0.12, 0.16, M.marble);
  ISO.panelLeft(
    g,
    proj,
    ix - 0.06,
    iy + id + 0.06,
    ez + 0.03,
    iw + 0.12,
    0.09,
    ISO.shade(M.blue, ISO.faceFactor("left")),
  );
  ISO.panelRight(
    g,
    proj,
    ix + iw + 0.06,
    iy - 0.06,
    ez + 0.03,
    id + 0.12,
    0.09,
    ISO.shade(M.red, ISO.faceFactor("right")),
  );
  // roof
  ISO.gableRoof(
    g,
    proj,
    ix - 0.06,
    iy - 0.06,
    ez + 0.16,
    iw + 0.12,
    id + 0.12,
    0.5 + 0.12 * L,
    M.terracotta,
    {
      ridge: "y",
      overhang: 0.2,
      tympanum: L >= 2 ? M.blue : undefined,
      outline: out,
    },
  );
  // front colonnade (closest)
  ISO.colonnade(
    g,
    proj,
    ix,
    iy + id,
    ix + iw,
    iy + id,
    topZ,
    colH,
    colR,
    frontN,
    M.marble,
    {},
  );
  // acroteria (gold) at apex
  if (L >= 3) {
    const ap = proj.p(ix + iw / 2, iy + id + 0.2, ez + 0.16 + 0.5 + 0.12 * L);
    g.circle(ap.x, ap.y - 2, 3.4).fill({ color: M.gold });
  }
  // altar + sacred flame in front
  if (L >= 2) {
    const ax = W / 2 - 0.2;
    const ay = D + 0.18;
    ISO.box(g, proj, ax, ay, 0, 0.4, 0.4, 0.32, M.stone, { outline: out });
    const ap = proj.p(ax + 0.2, ay + 0.2, 0.32);
    const fl = new Flame(ap.x, ap.y, 1.1 + 0.14 * L);
    c.addChild(fl.node);
    anims.push(fl);
  }
  // sacred grove: cypresses flanking + votive urns
  PROP.cypress(g, proj, -0.35, D - 0.2, 0, 1.15);
  PROP.cypress(g, proj, W + 0.35, D - 0.2, 0, 1.15);
  if (L >= 1) {
    PROP.urn(g, proj, 0.25, D + 0.05, 0, 1);
    PROP.urn(g, proj, W - 0.25, D + 0.05, 0, 1);
  }
  if (L >= 3) {
    PROP.cypress(g, proj, -0.3, D - 1.3, 0, 0.95);
    PROP.cypress(g, proj, W + 0.3, D - 1.3, 0, 0.95);
  }
  return { container: c, body: g, anims, foot: [W, D] };
};

// ====================== HOUSE ===========================================
const house: Builder = function (L, opt) {
  const cfg = [
    { W: 1, D: 1 },
    { W: 1, D: 1 },
    { W: 2, D: 2 },
    { W: 2, D: 2 },
    { W: 3, D: 3 },
  ][L];
  const { W, D } = cfg;
  const s = setup(W, D, opt);
  const { proj, g, c, out, bx, anims } = s;
  const look = saltLook(opt?.salt ?? 0);

  /**
   * Masonry stack flush on the roof near the ridge + kit Smoke at the flue lip.
   * `baseZ` is the roof-surface height under the stack (not the peak float).
   */
  function chimneyAt(gx: number, gy: number, baseZ: number, sc?: number): void {
    if (!look.hasChimney) return;
    const scale = sc ?? 1;
    const topZ = drawChimneyStack(bx, gx, gy, baseZ, scale, look.lateralDx);
    const p = proj.p(gx + look.lateralDx, gy, topZ);
    const sm = new Smoke(p.x, p.y, scale * 0.55);
    c.addChild(sm.node);
    anims.push(sm);
  }

  if (L === 0) {
    // kalybe: round-ish mud hut + thatch hip roof
    bx(0.15, 0.15, 0, 0.7, 0.7, 0.55, wallCol(M.mud, look));
    ISO.panelLeft(g, proj, 0.38, 0.85, 0, 0.24, 0.4, look.door);
    ISO.hipRoof(g, proj, 0.05, 0.05, 0.55, 0.9, 0.9, 0.5, ISO.shade(M.thatch, look.wallF), {
      overhang: 0.12,
      outline: out,
    });
    // Near hip ridge (centre), base slightly below peak so stack sits on slope
    chimneyAt(0.48, 0.42, 0.92, 0.7);
  } else if (L === 1) {
    // casa piccola: mudbrick box + tile gable
    bx(0.12, 0.12, 0, 0.76, 0.76, 0.7, wallCol(M.mud, look));
    ISO.panelLeft(g, proj, 0.4, 0.88, 0, 0.22, 0.46, look.door);
    ISO.panelLeft(g, proj, 0.18, 0.88, 0.42, 0.16, 0.16, look.accent);
    ISO.hipRoof(g, proj, 0.08, 0.08, 0.7, 0.8, 0.8, 0.46, look.roof, {
      overhang: 0.14,
      outline: out,
    });
    chimneyAt(0.38, 0.38, 1.02, 0.78);
  } else if (L === 2) {
    // casa: plastered 2x2, tile roof, windows
    bx(0.1, 0.1, 0, 1.8, 1.8, 0.95, wallCol(M.marbleWarm, look));
    ISO.panelLeft(g, proj, 0.75, 1.9, 0, 0.4, 0.62, look.door);
    // Window arrangement variants (same wall, different count/spacing)
    if (look.winMode === 0) {
      ISO.panelLeft(g, proj, 0.28, 1.9, 0.5, 0.3, 0.3, look.accent);
      ISO.panelLeft(g, proj, 1.28, 1.9, 0.5, 0.3, 0.3, look.accent);
    } else if (look.winMode === 1) {
      ISO.panelLeft(g, proj, 0.22, 1.9, 0.5, 0.28, 0.3, look.accent);
      ISO.panelLeft(g, proj, 0.78, 1.9, 0.5, 0.28, 0.3, look.accent);
      ISO.panelLeft(g, proj, 1.34, 1.9, 0.5, 0.28, 0.3, look.accent);
    } else {
      ISO.panelLeft(g, proj, 0.7, 1.9, 0.48, 0.4, 0.34, look.accent);
    }
    ISO.hipRoof(g, proj, 0.05, 0.05, 0.95, 1.9, 1.9, 0.66, look.roof, {
      overhang: 0.18,
      outline: out,
    });
    // Optional fabric awning strip under the eaves
    if (look.hasAwning) {
      ISO.panelLeft(g, proj, 0.2, 1.92, 0.88, 1.4, 0.08, ISO.shade(look.accent, 0.9));
    }
    // Inset from ridge peak toward back-left slope (not on eave edge)
    chimneyAt(0.55, 0.48, 1.42, 0.9);
  } else if (L === 3) {
    // megaron: two storeys
    bx(0.1, 0.1, 0, 1.8, 1.8, 1.0, wallCol(M.marbleWarm, look));
    bx(0.22, 0.22, 1.0, 1.56, 1.56, 0.9, wallCol(M.marble, look));
    // balcony band
    ISO.panelLeft(g, proj, 0.1, 1.9, 0.98, 1.8, 0.12, look.accent);
    ISO.panelLeft(g, proj, 0.4, 1.9, 0.1, 0.4, 0.62, look.door);
    const winXs =
      look.winMode === 1 ? [0.35, 0.85, 1.35] : look.winMode === 2 ? [0.75] : [0.45, 1.05];
    winXs.forEach((x) => ISO.panelLeft(g, proj, x, 1.9, 0.4, 0.3, 0.34, look.accent));
    const upXs = look.winMode === 2 ? [0.75] : [0.5, 1.05];
    upXs.forEach((x) =>
      ISO.panelLeft(g, proj, x, 1.78, 1.32, 0.28, 0.34, ISO.shade(look.accent, 0.85)),
    );
    ISO.gableRoof(g, proj, 0.18, 0.18, 1.9, 1.64, 1.64, 0.5, look.roof, {
      ridge: "y",
      overhang: 0.14,
      outline: out,
    });
    // On gable slope just off the y-ridge (rx = 0.18+0.82), mid-depth
    chimneyAt(0.92, 0.85, 2.28, 0.88);
  } else {
    // mnemeion with courtyard (ring of rooms)
    const wings = [
      [0.1, 0.1, 2.8, 0.7],
      [0.1, 2.2, 2.8, 0.7],
      [0.1, 0.8, 0.7, 1.4],
      [2.2, 0.8, 0.7, 1.4],
    ];
    // courtyard floor
    ISO.box(g, proj, 0.8, 0.8, 0, 1.4, 1.4, 0.04, ISO.shade(M.stone, 1.04), {
      outline: out,
    });
    // small inner colonnade
    ISO.colonnade(g, proj, 0.95, 2.05, 2.05, 2.05, 0.04, 0.7, 0.07, 4, M.marble, {});
    wings.forEach((w) => {
      bx(w[0], w[1], 0, w[2], w[3], 1.05, wallCol(M.marble, look));
      ISO.gableRoof(g, proj, w[0], w[1], 1.05, w[2], w[3], 0.38, look.roof, {
        ridge: w[2] > w[3] ? "x" : "y",
        overhang: 0.12,
        outline: out,
      });
    });
    ISO.panelLeft(g, proj, 1.2, 3.0, 0, 0.6, 0.66, look.door);
    // On north wing gable, near ridge (ridge-x for wide wing)
    chimneyAt(1.5, 0.35, 1.32, 0.85);
  }
  // dooryard greenery (Caesar-style lived-in plots)
  if (L >= 1) {
    PROP.bush(g, proj, 0.05, D - 0.05, 0, 0.7, 11);
    PROP.bush(g, proj, W - 0.05, D + 0.02, 0, 0.7, 23);
  }
  if (L === 2 || L === 3) {
    PROP.olive(g, proj, W + 0.18, D - 0.4, 0, 0.8);
    PROP.gardenBed(g, proj, 0.0, D - 0.02, W * 0.5, 0.22, 5);
  }
  if (L === 4) {
    PROP.cypress(g, proj, 1.5, 1.5, 0.04, 0.9);
    PROP.cypress(g, proj, 1.5, 1.5, 0.04, 0.9);
    PROP.gardenBed(g, proj, 0.85, 0.85, 1.3, 1.3, 7);
    PROP.urn(g, proj, 0.9, 2.05, 0.04, 0.8);
    PROP.urn(g, proj, 2.05, 2.05, 0.04, 0.8);
    PROP.olive(g, proj, W + 0.2, D - 0.5, 0, 0.85);
    PROP.bush(g, proj, -0.05, D - 0.6, 0, 0.7, 3);
  }
  return { container: c, body: g, anims, foot: [W, D] };
};

// ====================== FORTRESS ========================================
const fortress: Builder = function (L, opt) {
  const sizes = [
    [2, 2],
    [2, 2],
    [3, 3],
    [3, 4],
    [4, 4],
  ][L];
  const [W, D] = sizes;
  const s = setup(W, D, opt);
  const { proj, g, c, bx, anims } = s;
  const wallH = 1.0 + L * 0.1;
  const towerH = wallH + 0.7 + L * 0.12;

  // base plinth
  bx(0, 0, 0, W, D, 0.18, M.plinth);
  const z0 = 0.18;
  const wallT = 0.28; // wall thickness
  // perimeter walls (4 boxes)
  bx(0, 0, z0, W, wallT, wallH, M.stone); // back
  bx(0, D - wallT, z0, W, wallT, wallH, M.stone); // front
  bx(0, 0, z0, wallT, D, wallH, M.stone); // left
  bx(W - wallT, 0, z0, wallT, D, wallH, M.stone); // right
  // merlons (crenellations) along front + right
  const merl = (gx: number, gy: number, n: number, axis: string): void => {
    for (let i = 0; i < n; i++) {
      const t = (i + 0.5) / n;
      const mx = axis === "x" ? gx + W * t - 0.12 : gx;
      const my = axis === "y" ? gy + D * t - 0.12 : gy;
      bx(mx, my, z0 + wallH, 0.22, 0.22, 0.22, M.stone);
    }
  };
  merl(0, D - wallT + 0.03, Math.round(W * 2), "x");
  merl(W - wallT + 0.03, 0, Math.round(D * 2), "y");
  // keep (central tower)
  const kw = Math.max(0.9, W * 0.5);
  const kd = Math.max(0.9, D * 0.5);
  const kx = (W - kw) / 2;
  const ky = (D - kd) / 2;
  if (L >= 1) bx(kx, ky, z0, kw, kd, towerH, M.stone);
  // corner towers
  if (L >= 2) {
    const ct = 0.55;
    const ch = towerH + 0.2;
    [
      [0, 0],
      [W - ct, 0],
      [0, D - ct],
      [W - ct, D - ct],
    ].forEach((p) => bx(p[0], p[1], z0, ct, ct, ch, ISO.shade(M.stone, 1.02)));
  }
  // gate
  ISO.panelLeft(g, proj, W / 2 - 0.3, D, z0, 0.6, wallH * 0.7, ISO.shade(M.wood, 0.5));
  // banner on keep
  const fp = proj.p(kx + kw / 2, ky + kd / 2, z0 + towerH);
  const fg = new Flag(fp.x, fp.y, 1.1 + 0.06 * L, M.red);
  c.addChild(fg.node);
  anims.push(fg);
  return { container: c, body: g, anims, foot: [W, D] };
};

// ====================== TOWER ===========================================
const tower: Builder = function (L, opt) {
  const sizes = [
    [1, 1],
    [1, 1],
    [1, 1],
    [2, 2],
    [2, 2],
  ][L];
  const [W, D] = sizes;
  const s = setup(W, D, opt);
  const { proj, g, c, bx, anims } = s;
  const h = [1.7, 2.3, 2.9, 3.4, 4.2][L];
  const inset = 0.12;
  // tapered: base wider plinth
  bx(0, 0, 0, W, D, 0.16, M.plinth);
  bx(inset, inset, 0.16, W - 2 * inset, D - 2 * inset, h, M.stone);
  // string courses (bands)
  const bw = W - 2 * inset;
  for (let i = 1; i <= Math.floor(h); i++) {
    ISO.panelLeft(g, proj, inset, D - inset, 0.16 + i, bw, 0.06, ISO.shade(M.stone, 0.8));
  }
  // crenellated top: overhanging gallery
  bx(
    inset - 0.08,
    inset - 0.08,
    0.16 + h,
    W - 2 * inset + 0.16,
    D - 2 * inset + 0.16,
    0.26,
    ISO.shade(M.stone, 1.03),
  );
  const topZ = 0.16 + h + 0.26;
  const n = Math.round((W - 2 * inset) * 3);
  for (let i = 0; i < n; i++) {
    const t = (i + 0.5) / n;
    bx(
      inset - 0.08 + (W - 2 * inset + 0.16) * t - 0.08,
      D - inset + 0.0,
      topZ,
      0.16,
      0.16,
      0.2,
      M.stone,
    );
  }
  // arrow-slit windows
  [0.4, 1.1, 1.8, 2.5]
    .filter((z) => z < h - 0.3)
    .forEach((z) =>
      ISO.panelLeft(
        g,
        proj,
        W / 2 - 0.06,
        D - inset,
        0.16 + z,
        0.12,
        0.32,
        ISO.shade(M.ink, 1.4),
      ),
    );
  if (L >= 3) {
    const fp = proj.p(W / 2, D / 2, topZ + 0.2);
    const fg = new Flag(fp.x, fp.y, 0.95, M.red);
    c.addChild(fg.node);
    anims.push(fg);
  }
  return { container: c, body: g, anims, foot: [W, D] };
};

// ====================== LIGHTHOUSE (Pharos) =============================
const lighthouse: Builder = function (L, opt) {
  const [W, D] = [2, 2];
  const s = setup(W, D, opt);
  const { proj, g, c, out, bx, anims } = s;
  const baseH = [1.4, 1.7, 2.0, 2.3, 2.7][L];
  const midH = [0.0, 0.9, 1.2, 1.5, 1.9][L];
  const drumH = [0.6, 0.7, 0.8, 0.9, 1.0][L];
  // plinth steps
  const z0 = ISO.steps(g, proj, 0, 0, 0, W, D, 2, 0.14, 0.12, M.stone);
  // square base (tapered look via inset)
  bx(0.3, 0.3, z0, 1.4, 1.4, baseH, M.marble);
  let z = z0 + baseH;
  // octagonal mid tier
  if (midH > 0) {
    ISO.cylinder(g, proj, 1.0, 1.0, z, 0.5, midH, M.marbleWarm, {
      seg: 8,
      outline: out,
    });
    z += midH;
  }
  // round drum (lantern housing)
  ISO.cylinder(g, proj, 1.0, 1.0, z, 0.36, drumH, M.marble, { seg: 16, outline: out });
  z += drumH;
  // little colonnade ring around lantern
  if (L >= 2)
    ISO.colonnade(g, proj, 0.7, 1.0, 1.3, 1.0, z - drumH, drumH * 0.8, 0.05, 3, M.marble, {});
  // cap + statue
  bx(0.78, 0.78, z, 0.44, 0.44, 0.18, ISO.shade(M.bronze, 1.0));
  if (L >= 4) {
    const sp = proj.p(1.0, 1.0, z + 0.18);
    g.circle(sp.x, sp.y - 8, 4).fill({ color: M.gold });
  }
  // beacon fire at top
  const bp = proj.p(1.0, 1.0, z + 0.2);
  const bc = new Beacon(bp.x, bp.y, 1.0 + 0.08 * L);
  c.addChild(bc.node);
  anims.push(bc);
  return { container: c, body: g, anims, foot: [W, D] };
};

// ====================== MARKET (agora stalls + stoa) ====================
const market: Builder = function (L, opt) {
  const sizes = [
    [2, 2],
    [2, 3],
    [3, 3],
    [3, 4],
    [4, 4],
  ][L];
  const [W, D] = sizes;
  const s = setup(W, D, opt);
  const { proj, g, c, out, bx, anims } = s;
  const look = saltLook(opt?.salt ?? 0);
  // paved plot
  ISO.box(g, proj, 0, 0, 0, W, D, 0.05, ISO.shade(M.stone, 1.05), { outline: out });
  // back stoa: colonnade + roof along the back edge
  const stoaD = 0.7;
  bx(0, 0, 0.05, W, stoaD, 0.1, wallCol(M.marble, look));
  ISO.colonnade(
    g,
    proj,
    0.2,
    stoaD,
    W - 0.2,
    stoaD,
    0.15,
    0.95,
    0.08,
    Math.max(3, W + 1),
    M.marble,
    {},
  );
  bx(0, 0, 0.05 + 1.1, W, stoaD, 0.16, wallCol(M.marble, look));
  ISO.gableRoof(g, proj, 0, 0, 0.05 + 1.26, W, stoaD, 0.34, look.roof, {
    ridge: "x",
    overhang: 0.14,
    outline: out,
  });
  // market stalls with striped awnings
  const stalls = [
    [0.4, 1.2],
    [1.5, 1.2],
    [0.5, 2.1],
    [1.7, 2.2],
    [2.6, 1.4],
    [2.7, 2.5],
    [0.6, 3.0],
    [1.9, 3.1],
  ]
    .filter((p) => p[0] < W - 0.3 && p[1] < D - 0.2)
    .slice(0, [2, 3, 4, 6, 8][L]);
  // Salt rotates the awning stripe pair so two market variants don't match.
  const acc = [look.accent, ISO.shade(look.accent === M.red ? M.blue : M.red, 1)];
  stalls.forEach((p, i) => {
    bx(p[0], p[1], 0.05, 0.5, 0.5, 0.45, look.door);
    // awning: striped quad on top, slightly larger
    const a = proj.p(p[0] - 0.1, p[1] - 0.1, 0.62);
    const b = proj.p(p[0] + 0.65, p[1] - 0.1, 0.62);
    const cc = proj.p(p[0] + 0.65, p[1] + 0.6, 0.55);
    const d = proj.p(p[0] - 0.1, p[1] + 0.6, 0.55);
    ISO.poly(g, [a, b, cc, d], acc[i % 2]);
    ISO.poly(
      g,
      [
        a,
        proj.p(p[0] + 0.27, p[1] - 0.1, 0.62),
        proj.p(p[0] + 0.27, p[1] + 0.6, 0.55),
        d,
      ],
      ISO.shade(M.marble, 1.05),
    );
    // amphorae
    const ap = proj.p(p[0] + 0.6, p[1] + 0.55, 0.05);
    g.ellipse(ap.x, ap.y - 5, 4, 7).fill({ color: ISO.shade(look.roof, 0.9) });
  });
  PROP.cypress(g, proj, -0.25, D - 0.3, 0, 0.85);
  PROP.cypress(g, proj, W + 0.25, D - 0.3, 0, 0.85);
  PROP.amphora(g, proj, 0.2, D - 0.1, 0.05, 1, M.ochre);
  PROP.amphora(g, proj, 0.45, D - 0.15, 0.05, 0.9, look.roof);
  return { container: c, body: g, anims, foot: [W, D] };
};

// ====================== WAREHOUSE =======================================
const warehouse: Builder = function (L, opt) {
  const sizes = [
    [2, 2],
    [2, 3],
    [3, 3],
    [4, 3],
    [4, 4],
  ][L];
  const [W, D] = sizes;
  const s = setup(W, D, opt);
  const { proj, g, c, out, bx, anims } = s;
  const look = saltLook(opt?.salt ?? 0);
  ISO.box(g, proj, 0, 0, 0, W, D, 0.08, M.plinth, { outline: out });
  // central roofed depot
  const dw = W - 0.4;
  const dd = Math.min(D - 0.4, 1.6);
  bx(0.2, 0.2, 0.08, dw, dd, 0.9, wallCol(M.marbleWarm, look));
  ISO.gableRoof(g, proj, 0.15, 0.15, 0.98, dw + 0.1, dd + 0.1, 0.5, look.roof, {
    ridge: "x",
    overhang: 0.16,
    outline: out,
  });
  // open storage bays in front: amphora/sack stacks under little awnings
  const slotsY = D - 0.7;
  const cols = Math.round(W);
  for (let i = 0; i < cols; i++) {
    const x = 0.25 + (i * (W - 0.5)) / Math.max(1, cols - 1 || 1);
    // wooden frame
    bx(x, slotsY, 0.08, 0.12, 0.5, 0.6, look.door);
    // goods
    const gp = proj.p(x + 0.32, slotsY + 0.4, 0.08);
    for (let k = 0; k < 3; k++) {
      g.ellipse(gp.x + (k - 1) * 6, gp.y - 6 - k * 2, 4.5, 7).fill({
        color: ISO.shade(k % 2 ? look.roof : M.ochre, 0.95 - k * 0.08),
      });
    }
    // optional awning strip over every other bay
    if (look.hasAwning && i % 2 === 0) {
      ISO.panelLeft(g, proj, x - 0.05, slotsY + 0.55, 0.62, 0.55, 0.06, look.accent);
    }
  }
  // depot flag
  if (L >= 2) {
    const fp = proj.p(0.4, 0.4, 0.98 + 0.5 + 0.3);
    const fg = new Flag(fp.x, fp.y, 0.8, look.accent);
    c.addChild(fg.node);
    anims.push(fg);
  }
  return { container: c, body: g, anims, foot: [W, D] };
};

// ====================== WORKSHOP ========================================
const workshop: Builder = function (L, opt) {
  const sizes = [
    [1, 1],
    [2, 2],
    [2, 2],
    [3, 2],
    [3, 3],
  ][L];
  const [W, D] = sizes;
  const s = setup(W, D, opt);
  const { proj, g, c, out, bx, anims } = s;
  const look = saltLook(opt?.salt ?? 0);
  ISO.box(g, proj, 0, 0, 0, W, D, 0.07, M.plinth, { outline: out });
  // shed
  const sw = W - 0.3;
  const sd = D - 0.5;
  bx(0.15, 0.15, 0.07, sw, sd, 0.85, wallCol(M.mud, look));
  ISO.gableRoof(g, proj, 0.1, 0.1, 0.92, sw + 0.1, sd + 0.1, 0.42, look.roof, {
    ridge: "x",
    overhang: 0.14,
    outline: out,
  });
  // Door / shutter accent on the shed front
  ISO.panelLeft(g, proj, 0.3 + look.lateralDx, 0.15 + sd, 0.07, 0.28, 0.5, look.door);
  // kiln / furnace (stone, glowing mouth) in front-right, with chimney
  const nKiln = [1, 1, 2, 2, 3][L];
  for (let i = 0; i < nKiln; i++) {
    const kx = 0.25 + i * 0.85;
    const ky = D - 0.55;
    if (kx > W - 0.4) break;
    bx(kx, ky, 0.07, 0.5, 0.45, 0.7, wallCol(M.stone, look));
    // glowing mouth — kit bronze/ochre blend (no new raw hex)
    ISO.panelLeft(
      g,
      proj,
      kx + 0.12,
      ky + 0.45,
      0.12,
      0.26,
      0.22,
      ISO.mix(M.bronze, M.ochre, 0.55),
    );
    // masonry chimney + smoke (salt may drop the stack)
    if (look.hasChimney) {
      // Sit on kiln roof (body top ~0.77); small stack aligned with house look
      const cx = kx + 0.25;
      const cy = ky + 0.2;
      const topZ = drawChimneyStack(bx, cx, cy, 0.77, 0.85, look.lateralDx);
      const sp = proj.p(cx + look.lateralDx, cy, topZ);
      const sm = new Smoke(sp.x, sp.y, 0.55);
      c.addChild(sm.node);
      anims.push(sm);
    }
    // small flame at mouth
    const fp = proj.p(kx + 0.25, ky + 0.46, 0.16);
    const fl = new Flame(fp.x, fp.y, 0.55);
    c.addChild(fl.node);
    anims.push(fl);
  }
  // wood pile
  const wp = proj.p(0.3, D - 0.3, 0.07);
  for (let k = 0; k < 3; k++) {
    g.ellipse(wp.x - 6 + k * 5, wp.y - 3 - k * 3, 7, 3).fill({
      color: ISO.shade(look.door, 1 - k * 0.06),
    });
  }
  return { container: c, body: g, anims, foot: [W, D] };
};

// ====================== CONDUIT (aqueduct arcade) =======================
const conduit: Builder = function (L, opt) {
  const len = [2, 3, 3, 4, 5][L]; // spans along gy
  const [W, D] = [1, len];
  const s = setup(W, D, opt);
  const { proj, g, c, bx, anims } = s;
  const pierH = [0.9, 1.1, 1.5, 1.8, 2.2][L];
  const channelTop = pierH + 0.5;
  const pierW = 0.34;
  // piers along the length
  for (let i = 0; i <= len; i++) {
    bx(0.33, i - pierW / 2 < 0 ? 0 : i - pierW / 2, 0, pierW, pierW, pierH, M.stone);
  }
  // arch spandrels between piers (dark arch openings on the left face)
  for (let i = 0; i < len; i++) {
    // spandrel block
    bx(0.36, i + pierW / 2, pierH - 0.3, pierW * 0.85, 1 - pierW, 0.3, M.stone);
    // arch shadow
    const a = proj.p(0.36, i + pierW / 2, 0.1);
    const b = proj.p(0.36, i + 1 - pierW / 2, 0.1);
    const cc = proj.p(0.36, i + 0.5, pierH - 0.05);
    ISO.poly(g, [a, b, cc], ISO.shade(M.ink, 1.6), 0.85);
  }
  // top channel box
  bx(0.3, -0.05, pierH, 0.42, len + 0.1, 0.5, M.marbleWarm);
  // water surface along the channel (animated)
  const wy0 = 0.02;
  const wy1 = len + 0.02;
  const wz = pierH + 0.42;
  const wx0 = 0.36;
  const wx1 = 0.66;
  const wpts = [
    proj.p(wx0, wy0, wz),
    proj.p(wx1, wy0, wz),
    proj.p(wx1, wy1, wz),
    proj.p(wx0, wy1, wz),
  ];
  const w = new Water(wpts, 0.85);
  c.addChild(w.node);
  anims.push(w);
  void channelTop;
  return { container: c, body: g, anims, foot: [W, D] };
};

// ====================== BATHS ===========================================
const baths: Builder = function (L, opt) {
  const sizes = [
    [2, 2],
    [2, 3],
    [3, 3],
    [3, 4],
    [4, 4],
  ][L];
  const [W, D] = sizes;
  const s = setup(W, D, opt);
  const { proj, g, c, out, bx, anims } = s;
  const look = saltLook(opt?.salt ?? 0);
  ISO.box(g, proj, 0, 0, 0, W, D, 0.1, wallCol(M.stone, look), { outline: out });
  // enclosed hall at back
  const hd = Math.min(1.4, D * 0.5);
  bx(0.1, 0.0, 0.1, W - 0.2, hd, 1.0, wallCol(M.marbleWarm, look));
  ISO.gableRoof(g, proj, 0.05, -0.05, 1.1, W - 0.1, hd + 0.05, 0.5, look.roof, {
    ridge: "x",
    overhang: 0.16,
    tympanum: look.accent,
    outline: out,
  });
  // Small masonry stack on the hall gable (near ridge-x midspan), salt-gated
  if (look.hasChimney) {
    const cx = W * 0.5;
    const cy = hd * 0.45;
    // Roof zt=1.1, rh=0.5 → peak 1.6; base on slope near ridge
    const topZ = drawChimneyStack(bx, cx, cy, 1.42, 0.9, look.lateralDx);
    const sp = proj.p(cx + look.lateralDx, cy, topZ);
    const sm = new Smoke(sp.x, sp.y, 0.55);
    c.addChild(sm.node);
    anims.push(sm);
  }
  // colonnade framing the pool (front)
  const poolY0 = hd + 0.15;
  const poolY1 = D - 0.25;
  ISO.colonnade(
    g,
    proj,
    0.25,
    poolY0,
    W - 0.25,
    poolY0,
    0.1,
    0.9,
    0.07,
    Math.max(3, W),
    M.marble,
    {},
  );
  if (L >= 2) {
    ISO.colonnade(
      g,
      proj,
      0.25,
      poolY1 + 0.05,
      W - 0.25,
      poolY1 + 0.05,
      0.1,
      0.9,
      0.07,
      Math.max(3, W),
      M.marble,
      {},
    );
  }
  // sunken pool with water
  const px0 = 0.35;
  const px1 = W - 0.35;
  const pz = 0.06;
  const wpts = [
    proj.p(px0, poolY0 + 0.08, pz),
    proj.p(px1, poolY0 + 0.08, pz),
    proj.p(px1, poolY1, pz),
    proj.p(px0, poolY1, pz),
  ];
  // pool rim
  ISO.poly(
    g,
    [
      proj.p(px0 - 0.08, poolY0, 0.1),
      proj.p(px1 + 0.08, poolY0, 0.1),
      proj.p(px1 + 0.08, poolY1 + 0.08, 0.1),
      proj.p(px0 - 0.08, poolY1 + 0.08, 0.1),
    ],
    ISO.shade(M.marble, 0.9),
  );
  const w = new Water(wpts, 0.9);
  c.addChild(w.node);
  anims.push(w);
  PROP.urn(g, proj, 0.2, D - 0.1, 0.1, 1);
  PROP.urn(g, proj, W - 0.2, D - 0.1, 0.1, 1);
  PROP.cypress(g, proj, -0.3, D - 0.5, 0, 0.95);
  PROP.cypress(g, proj, W + 0.3, D - 0.5, 0, 0.95);
  return { container: c, body: g, anims, foot: [W, D] };
};

// ====================== THEATER (cavea + skene) =========================
const theater: Builder = function (L, opt) {
  const sizes = [
    [3, 2],
    [3, 3],
    [4, 3],
    [4, 4],
    [5, 4],
  ][L];
  const [W, D] = sizes;
  const s = setup(W, D, opt);
  const { proj, g, c, out } = s;
  const look = saltLook(opt?.salt ?? 0);
  const cx = W / 2;
  const cy = 0.2; // centre of the arcs (near back)
  const tiers = [3, 4, 5, 6, 7][L];
  const rMax = Math.min(W / 2 + 0.2, D - 0.4);
  const seatH = 0.16;
  // draw concentric seating rings from outer(back/top) to inner(front)
  for (let ti = tiers - 1; ti >= 0; ti--) {
    const r = 0.6 + (rMax - 0.6) * (ti / (tiers - 1));
    const z = ti * seatH;
    const seg = 22;
    // front-facing half-annulus (angles ~ from 0.15π to 0.85π => facing camera/front)
    const a0 = Math.PI * 0.12;
    const a1 = Math.PI * 0.88;
    const outer: ISO.Pt[] = [];
    const inner: ISO.Pt[] = [];
    for (let i = 0; i <= seg; i++) {
      const a = a0 + (a1 - a0) * (i / seg);
      outer.push(proj.p(cx + Math.cos(a) * r, cy + Math.sin(a) * r, z));
      inner.push(proj.p(cx + Math.cos(a) * (r - 0.34), cy + Math.sin(a) * (r - 0.34), z));
    }
    // riser (vertical front of the step)
    const riser: ISO.Pt[] = [];
    for (let i = 0; i <= seg; i++) {
      const a = a0 + (a1 - a0) * (i / seg);
      riser.push(proj.p(cx + Math.cos(a) * r, cy + Math.sin(a) * r, z - seatH));
    }
    // step top
    const top = inner.concat(outer.slice().reverse());
    const topFlat: number[] = [];
    top.forEach((p) => topFlat.push(p.x, p.y));
    g.poly(topFlat).fill({ color: ISO.shade(M.stone, 1.08 * look.wallF) });
    // riser face
    const rf = outer.concat(riser.slice().reverse());
    const rfFlat: number[] = [];
    rf.forEach((p) => rfFlat.push(p.x, p.y));
    g.poly(rfFlat).fill({ color: ISO.shade(M.stone, 0.74 * look.wallF) });
    if (out) {
      g.poly(topFlat).stroke({ width: 1, color: M.ink, alpha: 0.2 });
    }
  }
  // orchestra (round stage floor)
  const op: ISO.Pt[] = [];
  for (let i = 0; i <= 24; i++) {
    const a = (i / 24) * Math.PI * 2;
    op.push(proj.p(cx + Math.cos(a) * 0.5, cy + Math.sin(a) * 0.5, 0.01));
  }
  const opF: number[] = [];
  op.forEach((p) => opF.push(p.x, p.y));
  g.poly(opF).fill({ color: ISO.shade(M.stone, 1.14 * look.wallF) });
  // skene (stage building) at the very front
  ISO.box(g, proj, 0.4, D - 0.55, 0, W - 0.8, 0.4, 0.95, wallCol(M.marbleWarm, look), {
    outline: out,
  });
  ISO.colonnade(
    g,
    proj,
    0.6,
    D - 0.15,
    W - 0.6,
    D - 0.15,
    0.0,
    0.85,
    0.07,
    Math.max(3, W),
    M.marble,
    {},
  );
  ISO.box(g, proj, 0.35, D - 0.6, 0.95, W - 0.7, 0.5, 0.16, M.marble, { outline: out });
  PROP.cypress(g, proj, -0.3, D - 0.6, 0, 1.0);
  PROP.cypress(g, proj, W + 0.3, D - 0.6, 0, 1.0);
  PROP.statue(g, proj, 0.5, D - 0.05, 0, 0.8);
  PROP.statue(g, proj, W - 0.5, D - 0.05, 0, 0.8);
  return { container: c, body: g, anims: s.anims, foot: [W, D] };
};

// ====================== HARBOR ==========================================
const harbor: Builder = function (L, opt) {
  const sizes = [
    [2, 2],
    [3, 2],
    [3, 3],
    [4, 3],
    [4, 4],
  ][L];
  const [W, D] = sizes;
  const s = setup(W, D, opt);
  const { proj, g, c, out, bx, anims } = s;
  // water fills the whole plot; quay sits at the back
  const wpts = [
    proj.p(0, 0, 0),
    proj.p(W, 0, 0),
    proj.p(W, D, 0),
    proj.p(0, D, 0),
  ];
  const w = new Water(wpts, 0.95);
  c.addChildAt(w.node, 0);
  anims.push(w);
  // stone quay (back strip)
  const quayD = 0.9;
  bx(0, 0, 0, W, quayD, 0.3, M.stone);
  // harbor master house on the quay
  bx(0.2, 0.05, 0.3, 1.0, 0.6, 0.85, M.marbleWarm);
  ISO.gableRoof(g, proj, 0.15, 0.0, 1.15, 1.1, 0.65, 0.4, M.terracotta, {
    ridge: "x",
    overhang: 0.12,
    outline: out,
  });
  // wooden piers extending into the water (front)
  const piers = [1, 1, 2, 2, 3][L];
  for (let i = 0; i < piers; i++) {
    const px = 0.4 + (i * (W - 0.8)) / Math.max(1, piers);
    bx(px, quayD, 0.18, 0.3, D - quayD - 0.2, 0.12, M.wood);
    // bollards
    [quayD + 0.2, D - 0.4].forEach((py) =>
      bx(px - 0.04, py, 0.3, 0.12, 0.12, 0.18, M.woodLight),
    );
  }
  // crane (wooden A-frame) on the quay
  if (L >= 2) {
    const cb = proj.p(W - 0.6, quayD, 0.3);
    const ct = proj.p(W - 0.9, quayD + 0.6, 1.5);
    const arm = proj.p(W - 0.9, D - 0.3, 1.5);
    g.moveTo(cb.x, cb.y)
      .lineTo(ct.x, ct.y)
      .lineTo(arm.x, arm.y)
      .stroke({ width: 4, color: M.wood });
    g.circle(ct.x, ct.y, 3).fill({ color: M.wood });
    g.moveTo(arm.x, arm.y)
      .lineTo(arm.x, arm.y + 14)
      .stroke({ width: 1.5, color: M.ink, alpha: 0.6 });
  }
  PROP.amphora(g, proj, 1.4, 0.25, 0.3, 1, M.ochre);
  PROP.amphora(g, proj, 1.62, 0.3, 0.3, 0.9, M.terracotta);
  PROP.amphora(g, proj, 1.5, 0.5, 0.3, 0.95, M.terracotta);
  return { container: c, body: g, anims, foot: [W, D] };
};

// ====================== LIBRARY (two-storey Ionic stoa) =================
const library: Builder = function (L, opt) {
  const sizes = [
    [2, 2],
    [3, 2],
    [3, 3],
    [4, 3],
    [4, 3],
  ][L];
  const [W, D] = sizes;
  const s = setup(W, D, opt);
  const { proj, g, c, out, bx, anims } = s;
  const look = saltLook(opt?.salt ?? 0);
  const z0 = ISO.steps(g, proj, 0, 0, 0, W, D, 2, 0.12, 0.1, M.stone);
  const ins = 0.2;
  // main block
  bx(ins, ins, z0, W - 2 * ins, D - 2 * ins, 1.7, wallCol(M.marble, look));
  // niches (scroll shelves) on front
  for (let i = 0; i < Math.round(W); i++) {
    const x =
      ins + 0.25 + (i * (W - 2 * ins - 0.3)) / Math.max(1, Math.round(W) - 1 || 1);
    ISO.panelLeft(g, proj, x, D - ins, z0 + 0.3, 0.3, 0.5, ISO.shade(M.ink, 1.5));
    ISO.panelLeft(g, proj, x, D - ins, z0 + 1.0, 0.3, 0.5, ISO.shade(M.ink, 1.5));
  }
  // two-storey Ionic colonnade across the front porch
  ISO.colonnade(
    g,
    proj,
    ins + 0.1,
    D - 0.1,
    W - ins - 0.1,
    D - 0.1,
    z0,
    1.6,
    0.1,
    Math.max(4, W + 1),
    M.marble,
    { ionic: true },
  );
  // entablature + roof
  bx(ins - 0.05, ins - 0.05, z0 + 1.7, W - 2 * ins + 0.1, D - 2 * ins + 0.1, 0.18, M.marble);
  ISO.panelLeft(
    g,
    proj,
    ins - 0.05,
    D - ins + 0.05,
    z0 + 1.74,
    W - 2 * ins + 0.1,
    0.1,
    ISO.shade(look.accent, ISO.faceFactor("left")),
  );
  ISO.gableRoof(
    g,
    proj,
    ins - 0.05,
    ins - 0.05,
    z0 + 1.88,
    W - 2 * ins + 0.1,
    D - 2 * ins + 0.1,
    0.5,
    look.roof,
    { ridge: "y", overhang: 0.18, tympanum: look.accent, outline: out },
  );
  PROP.statue(g, proj, 0.4, D - 0.05, z0, 0.8);
  PROP.statue(g, proj, W - 0.4, D - 0.05, z0, 0.8);
  PROP.cypress(g, proj, -0.3, D - 0.5, 0, 0.95);
  PROP.cypress(g, proj, W + 0.3, D - 0.5, 0, 0.95);
  return { container: c, body: g, anims, foot: [W, D] };
};

// ====================== TOWNHALL (bouleuterion) =========================
const townhall: Builder = function (L, opt) {
  const sizes = [
    [2, 2],
    [3, 3],
    [3, 3],
    [4, 4],
    [4, 5],
  ][L];
  const [W, D] = sizes;
  const s = setup(W, D, opt);
  const { proj, g, c, out, bx, anims } = s;
  const look = saltLook(opt?.salt ?? 0);
  const z0 = ISO.steps(g, proj, 0, 0, 0, W, D, 2, 0.13, 0.1, M.stone);
  const ins = 0.25;
  const bodyH = 1.2 + L * 0.12;
  // main hall
  bx(ins, ins, z0, W - 2 * ins, D - 2 * ins - 0.4, bodyH, wallCol(M.marbleWarm, look));
  // hipped roof
  ISO.hipRoof(
    g,
    proj,
    ins - 0.05,
    ins - 0.05,
    z0 + bodyH,
    W - 2 * ins + 0.1,
    D - 2 * ins - 0.4 + 0.1,
    0.7 + 0.1 * L,
    look.roof,
    { overhang: 0.18, outline: out },
  );
  // front porch colonnade + small pediment
  const porchY = D - ins - 0.1;
  ISO.colonnade(
    g,
    proj,
    ins + 0.15,
    porchY,
    W - ins - 0.15,
    porchY,
    z0,
    1.0,
    0.09,
    Math.max(3, W),
    M.marble,
    {},
  );
  bx(ins + 0.05, porchY - 0.05, z0 + 1.0, W - 2 * ins - 0.1, 0.32, 0.16, M.marble);
  ISO.gableRoof(
    g,
    proj,
    ins + 0.05,
    porchY - 0.1,
    z0 + 1.16,
    W - 2 * ins - 0.1,
    0.42,
    0.34,
    look.roof,
    { ridge: "x", overhang: 0.12, pediment: M.marble, tympanum: M.gold, outline: out },
  );
  // door
  ISO.panelLeft(
    g,
    proj,
    W / 2 - 0.25,
    D - ins - 0.4,
    z0,
    0.5,
    bodyH * 0.6,
    ISO.shade(M.bronze, 0.85),
  );
  // civic banner on the ridge
  const fp = proj.p(W / 2, (ins + (D - 0.4)) / 2, z0 + bodyH + 0.7 + 0.1 * L);
  const fg = new Flag(fp.x, fp.y, 1.1, M.gold);
  c.addChild(fg.node);
  anims.push(fg);
  PROP.statue(g, proj, 0.45, D - 0.05, z0, 0.85);
  PROP.statue(g, proj, W - 0.45, D - 0.05, z0, 0.85);
  PROP.urn(g, proj, 0.2, D + 0.05, 0, 0.9);
  PROP.urn(g, proj, W - 0.2, D + 0.05, 0, 0.9);
  return { container: c, body: g, anims, foot: [W, D] };
};

// ====================== UNKNOWN (fallback) ==============================
const unknown: Builder = function (L, opt) {
  const [W, D] = [1, 1];
  const s = setup(W, D, opt);
  const { proj, g, c, out, bx } = s;
  const look = saltLook(opt?.salt ?? 0);
  // striped placeholder plinth
  ISO.box(g, proj, 0, 0, 0, 1, 1, 0.1, ISO.shade(M.stone, 0.96 * look.wallF), {
    outline: true,
  });
  // hatched top
  for (let i = -2; i < 6; i++) {
    const a = proj.p(i * 0.18, 0, 0.1);
    const b = proj.p(i * 0.18 + 0.9, 0.9, 0.1);
    g.moveTo(a.x, a.y)
      .lineTo(b.x, b.y)
      .stroke({ width: 3, color: ISO.shade(M.groundEdge, 1.0), alpha: 0.5 });
  }
  // crate — wood tone shifts with salt
  bx(0.18, 0.18, 0.1, 0.64, 0.64, 0.7, look.door, { outline: true });
  // big "?" mark
  const qp = proj.p(0.5, 0.5, 0.82);
  const t = new Text({
    text: "?",
    style: {
      fontFamily: "Georgia, serif",
      fontSize: 34,
      fill: 0xf4efe6,
      fontWeight: "700",
    },
  });
  t.anchor.set(0.5, 1);
  t.position.set(qp.x, qp.y - 2);
  c.addChild(t);
  void L;
  void out;
  return { container: c, body: g, anims: s.anims, foot: [W, D] };
};

export const BUILDERS: Record<string, Builder> = {
  temple,
  house,
  fortress,
  tower,
  lighthouse,
  market,
  warehouse,
  workshop,
  conduit,
  baths,
  theater,
  harbor,
  library,
  townhall,
  unknown,
};
