/* =========================================================================
   detail.ts — Mediterranean props & greenery (faithful port of detail.js)
   Cypress, bushes, olive trees, garden beds, amphorae, statues, urns,
   fountains, hedges. Drawn at a projected ground point (screen-space
   billboards) so they read crisply at any building scale. The "lived-in"
   Caesar-III layer.

   PORTED 1:1 from Polis-handoff/polis/project/js/detail.js (PixiJS v7) → v8/TS.

   DETERMINISM: the source already uses a SEEDED sin-hash `rnd(seed)` (NOT
   Math.random) for all static scatter (bush blobs, gardenBed flowers, hedge),
   so prop placement is already reproducible. That seeded rnd is kept verbatim,
   so the look is identical AND the city re-scan is deterministic.
   ========================================================================= */

import { Graphics } from "pixi.js";
import {
  MAT,
  shade as S,
  poly as isoPoly,
  outlinePoly,
  line as isoLine,
  lerp,
  type Proj,
} from "./iso";
import { CONTACT_SHADOW } from "../contactShadow";

const M = MAT;
const rnd = (seed: number): number => {
  const x = Math.sin(seed * 99.13) * 43758.5;
  return x - Math.floor(x);
};

export function cypress(
  g: Graphics,
  proj: Proj,
  gx: number,
  gy: number,
  z?: number,
  sc?: number,
): void {
  sc = sc || 1;
  const p = proj.p(gx, gy, z || 0);
  g.ellipse(
    p.x + CONTACT_SHADOW.offsetX * sc,
    p.y + CONTACT_SHADOW.offsetY * sc,
    8 * sc,
    3 * sc,
  ).fill({ color: M.shadow, alpha: CONTACT_SHADOW.alpha });
  g.rect(p.x - 1.2 * sc, p.y - 8 * sc, 2.4 * sc, 8 * sc).fill({ color: M.woodDk });
  g.ellipse(p.x, p.y - 22 * sc, 6 * sc, 20 * sc).fill({ color: M.cypressDk });
  g.ellipse(p.x - 1.6 * sc, p.y - 24 * sc, 4 * sc, 18 * sc).fill({ color: M.cypress });
  g.ellipse(p.x - 2.4 * sc, p.y - 28 * sc, 1.8 * sc, 9 * sc).fill({
    color: S(M.cypress, 1.18),
  });
}

export function bush(
  g: Graphics,
  proj: Proj,
  gx: number,
  gy: number,
  z?: number,
  sc?: number,
  seed?: number,
): void {
  sc = sc || 1;
  seed = seed || gx * 3 + gy;
  const p = proj.p(gx, gy, z || 0);
  g.ellipse(
    p.x + CONTACT_SHADOW.offsetX * sc,
    p.y + CONTACT_SHADOW.offsetY * sc,
    9 * sc,
    3.2 * sc,
  ).fill({ color: M.shadow, alpha: CONTACT_SHADOW.alpha });
  const blobs = [
    [0, 0, 7],
    [-5, 2, 5],
    [5, 1, 5.5],
    [1, -4, 6],
  ];
  blobs.forEach((b, i) => {
    g.circle(
      p.x + b[0] * sc + (rnd(seed + i) - 0.5) * 2,
      p.y - 4 * sc - b[1] * sc,
      b[2] * sc,
    ).fill({ color: i % 2 ? M.bush : M.leafDk });
  });
  g.circle(p.x - 3 * sc, p.y - 8 * sc, 3 * sc).fill({
    color: S(M.leafLt, 1.05),
    alpha: 0.9,
  });
}

export function olive(
  g: Graphics,
  proj: Proj,
  gx: number,
  gy: number,
  z?: number,
  sc?: number,
): void {
  sc = sc || 1;
  const p = proj.p(gx, gy, z || 0);
  g.ellipse(
    p.x + CONTACT_SHADOW.offsetX * sc,
    p.y + CONTACT_SHADOW.offsetY * sc,
    12 * sc,
    4 * sc,
  ).fill({ color: M.shadow, alpha: CONTACT_SHADOW.alpha });
  g.moveTo(p.x, p.y)
    .lineTo(p.x - 1 * sc, p.y - 11 * sc)
    .stroke({ width: 2.4 * sc, color: M.woodDk });
  [
    [0, -16, 11],
    [-7, -13, 7],
    [7, -14, 7],
    [0, -22, 8],
  ].forEach((b, i) => {
    g.circle(p.x + b[0] * sc, p.y + b[1] * sc, b[2] * sc).fill({
      color: i % 2 ? M.leaf : M.leafDk,
    });
  });
  g.circle(p.x - 4 * sc, p.y - 20 * sc, 4 * sc).fill({ color: M.leafLt, alpha: 0.9 });
}

// low planted garden bed across a footprint area (world coords)
export function gardenBed(
  g: Graphics,
  proj: Proj,
  x0: number,
  y0: number,
  w: number,
  d: number,
  seed?: number,
  // A4 — optional real-art base (repeating grass texture fill); flowers and
  // outline still draw on top. Pixi's FillInput: kit stays spriteAssets-free.
  baseFill?: import("pixi.js").FillInput | null,
): void {
  seed = seed || 1;
  const a = proj.p(x0, y0, 0.02);
  const b = proj.p(x0 + w, y0, 0.02);
  const c = proj.p(x0 + w, y0 + d, 0.02);
  const e = proj.p(x0, y0 + d, 0.02);
  // Low-alpha base so the continuous meadow shows through (flowers carry the
  // garden identity, not a bright green slab). Matches ALPHA.fieldParcel /
  // ALPHA.fieldBorder in palette.ts.
  if (baseFill) {
    g.poly([a.x, a.y, b.x, b.y, c.x, c.y, e.x, e.y]).fill(baseFill);
  } else {
    isoPoly(g, [a, b, c, e], M.ground, 0.15);
  }
  outlinePoly(g, [a, b, c, e], S(M.ground, 0.9), 1.2, 0.22);
  const cols = [M.flowerA, M.flowerB, M.flowerC];
  for (let i = 0; i < Math.round(w * d * 8); i++) {
    const u = rnd(seed + i);
    const v = rnd(seed + i + 50);
    const pt = proj.p(x0 + 0.1 + u * (w - 0.2), y0 + 0.1 + v * (d - 0.2), 0.02);
    g.circle(pt.x, pt.y, 1.5).fill({ color: cols[i % 3], alpha: 0.95 });
  }
}

export function hedge(
  g: Graphics,
  proj: Proj,
  gx0: number,
  gy0: number,
  gx1: number,
  gy1: number,
  n: number,
  z?: number,
): void {
  for (let i = 0; i <= n; i++) {
    const t = i / n;
    bush(g, proj, gx0 + (gx1 - gx0) * t, gy0 + (gy1 - gy0) * t, z || 0, 0.6, i * 7);
  }
}

export function amphora(
  g: Graphics,
  proj: Proj,
  gx: number,
  gy: number,
  z?: number,
  sc?: number,
  color?: number,
): void {
  sc = sc || 1;
  color = color || M.terracotta;
  const p = proj.p(gx, gy, z || 0);
  g.ellipse(
    p.x + CONTACT_SHADOW.offsetX * sc,
    p.y + CONTACT_SHADOW.offsetY * sc,
    4 * sc,
    1.6 * sc,
  ).fill({ color: M.shadow, alpha: CONTACT_SHADOW.alpha });
  g.ellipse(p.x, p.y - 7 * sc, 4 * sc, 7 * sc).fill({ color: S(color, 0.92) });
  g.ellipse(p.x - 1.3 * sc, p.y - 8 * sc, 1.6 * sc, 4.5 * sc).fill({
    color: S(color, 1.12),
  });
  g.rect(p.x - 1.4 * sc, p.y - 15 * sc, 2.8 * sc, 4 * sc).fill({ color: S(color, 0.78) });
  g.moveTo(p.x - 3.6 * sc, p.y - 13 * sc)
    .lineTo(p.x - 2 * sc, p.y - 10 * sc)
    .moveTo(p.x + 3.6 * sc, p.y - 13 * sc)
    .lineTo(p.x + 2 * sc, p.y - 10 * sc)
    .stroke({ width: 1, color: S(color, 0.7) });
}

export function urn(
  g: Graphics,
  proj: Proj,
  gx: number,
  gy: number,
  z?: number,
  sc?: number,
): void {
  sc = sc || 1;
  const p = proj.p(gx, gy, z || 0);
  g.rect(p.x - 3 * sc, p.y - 4 * sc, 6 * sc, 4 * sc).fill({ color: S(M.marble, 0.9) });
  g.ellipse(p.x, p.y - 10 * sc, 5 * sc, 6 * sc).fill({ color: M.marble });
  g.circle(p.x - 2 * sc, p.y - 15 * sc, 3 * sc).fill({ color: M.bush });
  g.circle(p.x + 2 * sc, p.y - 14 * sc, 3 * sc).fill({ color: M.bush });
  g.circle(p.x, p.y - 17 * sc, 3 * sc).fill({ color: M.bush });
}

export function statue(
  g: Graphics,
  proj: Proj,
  gx: number,
  gy: number,
  z?: number,
  sc?: number,
  mat?: number,
): void {
  sc = sc || 1;
  mat = mat || M.marble;
  const p = proj.p(gx, gy, z || 0);
  g.ellipse(
    p.x + CONTACT_SHADOW.offsetX * sc,
    p.y + CONTACT_SHADOW.offsetY * sc,
    7 * sc,
    2.4 * sc,
  ).fill({ color: M.shadow, alpha: CONTACT_SHADOW.alpha });
  g.rect(p.x - 4 * sc, p.y - 7 * sc, 8 * sc, 7 * sc).fill({ color: S(mat, 0.84) });
  g.rect(p.x - 4.6 * sc, p.y - 8.4 * sc, 9.2 * sc, 1.6 * sc).fill({ color: S(mat, 1.0) });
  // figure
  g.rect(p.x - 2.6 * sc, p.y - 22 * sc, 5.2 * sc, 14 * sc).fill({ color: S(mat, 1.08) });
  g.rect(p.x + 0.4 * sc, p.y - 22 * sc, 2.2 * sc, 14 * sc).fill({ color: S(mat, 0.9) });
  g.circle(p.x, p.y - 24 * sc, 2.6 * sc).fill({ color: S(mat, 1.12) });
}

export function fountain(
  g: Graphics,
  proj: Proj,
  gx: number,
  gy: number,
  z?: number,
  sc?: number,
): void {
  sc = sc || 1;
  const p = proj.p(gx, gy, z || 0);
  g.ellipse(p.x, p.y, 13 * sc, 6.5 * sc).fill({ color: S(M.marble, 0.86) });
  g.ellipse(p.x, p.y - 1 * sc, 10 * sc, 4.8 * sc).fill({ color: M.water });
  g.ellipse(p.x - 2 * sc, p.y - 2 * sc, 5 * sc, 2 * sc).fill({
    color: S(M.water, 1.3),
    alpha: 0.6,
  });
  g.rect(p.x - 1.4 * sc, p.y - 10 * sc, 2.8 * sc, 9 * sc).fill({ color: M.marble });
  g.ellipse(p.x, p.y - 10 * sc, 4 * sc, 1.8 * sc).fill({ color: S(M.marble, 1.1) });
}

// a paved path strip (road feel) along gy at gx
export function pavers(
  g: Graphics,
  proj: Proj,
  x0: number,
  y0: number,
  w: number,
  d: number,
): void {
  const a = proj.p(x0, y0, 0.015);
  const b = proj.p(x0 + w, y0, 0.015);
  const c = proj.p(x0 + w, y0 + d, 0.015);
  const e = proj.p(x0, y0 + d, 0.015);
  isoPoly(g, [a, b, c, e], M.stone);
  isoPoly(g, [a, b, c, e], S(M.stone, 1.06), 0.5);
  for (let i = 1; i < Math.round(d * 2); i++) {
    const t = i / Math.round(d * 2);
    isoLine(g, lerp(a, e, t), lerp(b, c, t), S(M.stone, 0.78), 1, 0.4);
  }
}

/** PROP namespace mirror of the source's `global.PROP`. */
export const PROP = {
  cypress,
  bush,
  olive,
  gardenBed,
  hedge,
  amphora,
  urn,
  statue,
  fountain,
  pavers,
};
