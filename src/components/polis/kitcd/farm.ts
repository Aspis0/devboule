/* =========================================================================
   farm.ts — Caesar-III-style farmland primitives (decorative, static)
   Crop rows, vineyards, orchards, fallow fields, haystacks, farm sheds.
   Drawn at a projected ground point like detail.ts props.

   DETERMINISM: seeded by (seed) parameter — no Math.random/Date.now.
   PERFORMANCE: drawn ONCE into Graphics at build time; no per-frame work.
   ========================================================================= */

import { Graphics } from "pixi.js";
import { MAT, shade as S, poly as isoPoly, outlinePoly, type Proj } from "./iso";
import { olive } from "./detail";

/**
 * Optional real-art base fill (A4): when provided, the parcel's flat base
 * quad is painted with this style (a repeating texture from the sprite bank)
 * instead of the flat MAT color; all detail work (furrows, vines, trees,
 * tufts) still draws on top. Null/undefined ⇒ the original flat base.
 * Type is pixi's own FillInput to avoid importing spriteAssets from the kit.
 */
export type ParcelBaseFill = import("pixi.js").FillInput;

/** Paint the parcel base quad: textured when `baseFill` given, flat otherwise. */
function parcelBase(
  g: Graphics,
  pts: { x: number; y: number }[],
  baseFill: ParcelBaseFill | null | undefined,
  flatColor: number,
): void {
  if (baseFill) {
    g.poly(pts.flatMap((p) => [p.x, p.y])).fill(baseFill);
  } else {
    isoPoly(g, pts, flatColor);
  }
}

// Seeded sin-hash (same as detail.ts).
const rnd = (seed: number): number => {
  const x = Math.sin(seed * 99.13) * 43758.5;
  return x - Math.floor(x);
};

/** Draw a subtle earth-tone boundary ridge around a parcel rect. */
function drawParcelBorder(
  g: Graphics,
  proj: Proj,
  gx: number,
  gy: number,
  w: number,
  d: number,
): void {
  const a = proj.p(gx, gy, 0.01);
  const b = proj.p(gx + w, gy, 0.01);
  const c = proj.p(gx + w, gy + d, 0.01);
  const e = proj.p(gx, gy + d, 0.01);
  outlinePoly(g, [a, b, c, e], S(MAT.earthDk, 0.85), 1.2, 0.45);
}

/**
 * cropRows — parallel furrow rows across the parcel.
 * 3–5 rows per tile width, alternating earth-dark / green-sprout strips.
 */
export function cropRows(
  g: Graphics,
  proj: Proj,
  gx: number,
  gy: number,
  w: number,
  d: number,
  seed: number,
  baseFill?: ParcelBaseFill | null,
): void {
  drawParcelBorder(g, proj, gx, gy, w, d);
  // Base earth fill.
  const a = proj.p(gx, gy, 0.015);
  const b = proj.p(gx + w, gy, 0.015);
  const c = proj.p(gx + w, gy + d, 0.015);
  const e = proj.p(gx, gy + d, 0.015);
  parcelBase(g, [a, b, c, e], baseFill, S(MAT.earth, 0.95));

  // Furrow rows along the parcel's x-axis (long axis).
  const rowCount = Math.round(w * 3.5 + rnd(seed) * 1.5);
  for (let i = 0; i < rowCount; i++) {
    const t = (i + 0.5) / rowCount;
    const rowGy = gy + t * d;
    const rowA = proj.p(gx + 0.15, rowGy, 0.02);
    const rowB = proj.p(gx + w - 0.15, rowGy, 0.02);
    const isSprout = i % 2 === 0;
    const jitterX = rnd(seed + i * 7) * 0.08 - 0.04;
    const jitterY = rnd(seed + i * 13) * 0.04 - 0.02;
    g.moveTo(rowA.x + jitterX, rowA.y + jitterY)
      .lineTo(rowB.x + jitterX, rowB.y + jitterY)
      .stroke({
        width: isSprout ? 2.2 : 1.6,
        color: isSprout ? S(MAT.grass, 1.05) : S(MAT.earthDk, 0.92),
        alpha: isSprout ? 0.85 : 0.6,
      });
  }
  // Sprout dots on alternating rows.
  for (let i = 0; i < rowCount; i += 2) {
    const t = (i + 0.5) / rowCount;
    const sprouts = Math.round(w * 2.5);
    for (let j = 0; j < sprouts; j++) {
      const sx = gx + 0.2 + (j / sprouts) * (w - 0.4);
      const sy = gy + t * d + rnd(seed + i * 31 + j * 17) * 0.2 - 0.1;
      const pt = proj.p(sx, sy, 0.025);
      const r = 1.2 + rnd(seed + i + j * 23) * 0.8;
      g.circle(pt.x, pt.y, r).fill({
        color: rnd(seed + i + j) > 0.5 ? MAT.grass : MAT.leaf,
        alpha: 0.8,
      });
    }
  }
}

/**
 * vineyard — rows of small posts + vine arcs (short vertical stems with
 * darker green blobs), sparser than cropRows.
 */
export function vineyard(
  g: Graphics,
  proj: Proj,
  gx: number,
  gy: number,
  w: number,
  d: number,
  seed: number,
  baseFill?: ParcelBaseFill | null,
): void {
  drawParcelBorder(g, proj, gx, gy, w, d);
  // Bare earth base.
  const a = proj.p(gx, gy, 0.015);
  const b = proj.p(gx + w, gy, 0.015);
  const c = proj.p(gx + w, gy + d, 0.015);
  const e = proj.p(gx, gy + d, 0.015);
  parcelBase(g, [a, b, c, e], baseFill, S(MAT.sand, 0.9));

  // Vine rows — fewer than crops (1 per ~1.5 tiles of depth).
  const rowCount = Math.max(2, Math.round(d / 1.5));
  for (let i = 0; i < rowCount; i++) {
    const t = (i + 0.5) / rowCount;
    const rowGy = gy + t * d;
    // Posts along the row.
    const postCount = Math.max(2, Math.round(w / 1.2));
    for (let j = 0; j < postCount; j++) {
      const px = gx + 0.3 + (j / (postCount - 1)) * (w - 0.6);
      const pBase = proj.p(px, rowGy, 0);
      const pTop = proj.p(px, rowGy, 0.08);
      // Post.
      g.moveTo(pBase.x, pBase.y)
        .lineTo(pTop.x, pTop.y)
        .stroke({ width: 1.2, color: S(MAT.woodDk, 0.9) });
      // Vine blob on top.
      const blobR = 2.2 + rnd(seed + i * 11 + j * 7) * 1.2;
      g.circle(pTop.x, pTop.y - 1.5, blobR).fill({
        color: rnd(seed + i + j) > 0.4 ? MAT.leafDk : MAT.bush,
        alpha: 0.88,
      });
    }
    // Wire/arc between posts.
    if (postCount >= 2) {
      const firstPost = proj.p(gx + 0.3, rowGy, 0.07);
      const lastPost = proj.p(gx + w - 0.3, rowGy, 0.07);
      g.moveTo(firstPost.x, firstPost.y)
        .lineTo(lastPost.x, lastPost.y)
        .stroke({ width: 0.6, color: S(MAT.wood, 0.8), alpha: 0.5 });
    }
  }
}

/**
 * orchardGrid — trees on a regular lattice (1 tree per ~1.5 tiles).
 * Reuses olive() from detail.ts with small scale variance from seed.
 */
export function orchardGrid(
  g: Graphics,
  proj: Proj,
  gx: number,
  gy: number,
  w: number,
  d: number,
  seed: number,
  baseFill?: ParcelBaseFill | null,
): void {
  drawParcelBorder(g, proj, gx, gy, w, d);
  // Light grass base.
  const a = proj.p(gx, gy, 0.015);
  const b = proj.p(gx + w, gy, 0.015);
  const c = proj.p(gx + w, gy + d, 0.015);
  const e = proj.p(gx, gy + d, 0.015);
  parcelBase(g, [a, b, c, e], baseFill, S(MAT.grassDk, 0.95));

  // Tree grid — ~1 tree per 1.5 tiles.
  const spacing = 1.5;
  const cols = Math.max(1, Math.floor(w / spacing));
  const rows = Math.max(1, Math.floor(d / spacing));
  for (let row = 0; row < rows; row++) {
    for (let col = 0; col < cols; col++) {
      const tx = gx + 0.5 + (col + 0.5) * (w / cols) * 0.85;
      const ty = gy + 0.5 + (row + 0.5) * (d / rows) * 0.85;
      const scaleVar = 0.8 + rnd(seed + row * 13 + col * 7) * 0.35;
      olive(g, proj, tx, ty, 0, scaleVar);
    }
  }
}

/**
 * fallowField — bare earth tint quad + sparse dry-grass tufts + rocks.
 */
export function fallowField(
  g: Graphics,
  proj: Proj,
  gx: number,
  gy: number,
  w: number,
  d: number,
  seed: number,
  baseFill?: ParcelBaseFill | null,
): void {
  drawParcelBorder(g, proj, gx, gy, w, d);
  // Bare earth base.
  const a = proj.p(gx, gy, 0.015);
  const b = proj.p(gx + w, gy, 0.015);
  const c = proj.p(gx + w, gy + d, 0.015);
  const e = proj.p(gx, gy + d, 0.015);
  parcelBase(g, [a, b, c, e], baseFill, S(MAT.earth, 0.88));

  // Sparse dry-grass tufts.
  const tufts = Math.round(w * d * 3);
  for (let i = 0; i < tufts; i++) {
    const u = rnd(seed + i * 37);
    const v = rnd(seed + i * 53 + 100);
    const tx = gx + 0.15 + u * (w - 0.3);
    const ty = gy + 0.15 + v * (d - 0.3);
    const pt = proj.p(tx, ty, 0.02);
    const h = 3 + rnd(seed + i) * 3;
    // Dry tuft — a few short lines.
    for (let k = 0; k < 3; k++) {
      const angle = rnd(seed + i * 7 + k * 31) * 0.6 - 0.3;
      g.moveTo(pt.x, pt.y)
        .lineTo(pt.x + Math.sin(angle) * h * 0.6, pt.y - h)
        .stroke({
          width: 0.8,
          color: rnd(seed + i + k) > 0.5 ? S(MAT.thatch, 0.9) : S(MAT.earthDk, 0.85),
          alpha: 0.65,
        });
    }
  }

  // A few rocks (simplified from props.ts drawRocks).
  const rocks = Math.round(w * d * 0.8);
  for (let i = 0; i < rocks; i++) {
    const rx = gx + 0.2 + rnd(seed + i * 41) * (w - 0.4);
    const ry = gy + 0.2 + rnd(seed + i * 59) * (d - 0.4);
    const pt = proj.p(rx, ry, 0.01);
    const s = 2 + rnd(seed + i * 23) * 2.5;
    // Shadow.
    g.ellipse(pt.x + 1.5, pt.y + s * 0.35, s * 0.9, s * 0.35).fill({
      color: S(MAT.shadow, 0.8),
      alpha: 0.2,
    });
    // Stone body.
    const rot = rnd(seed + i * 17) * Math.PI;
    const pts: number[] = [];
    const n = 5;
    for (let k = 0; k < n; k++) {
      const angle = rot + (k / n) * Math.PI * 2;
      const rr = s * (0.6 + (k % 2) * 0.35);
      pts.push(pt.x + Math.cos(angle) * rr, pt.y + Math.sin(angle) * rr * 0.5);
    }
    g.poly(pts).fill({ color: rnd(seed + i) > 0.5 ? MAT.stone : S(MAT.stone, 0.85) });
  }
}

/**
 * haystack — single-tile round stack, straw tones, 2 ellipse layers + top knob.
 */
export function haystack(
  g: Graphics,
  proj: Proj,
  gx: number,
  gy: number,
  seed: number,
): void {
  const p = proj.p(gx + 0.5, gy + 0.5, 0);
  // Shadow.
  g.ellipse(p.x + 3, p.y + 2, 14, 6).fill({ color: S(MAT.shadow, 0.8), alpha: 0.22 });
  // Base layer.
  g.ellipse(p.x, p.y - 3, 12, 8).fill({ color: S(MAT.thatch, 0.92) });
  // Top layer (slightly smaller, lighter).
  g.ellipse(p.x - 1, p.y - 8, 9, 6).fill({ color: S(MAT.thatch, 1.08) });
  // Top knob.
  g.ellipse(p.x - 1, p.y - 13, 5, 3.5).fill({ color: S(MAT.thatchDk, 0.95) });
  // Straw wisps.
  for (let i = 0; i < 4; i++) {
    const angle = rnd(seed + i * 19) * Math.PI * 2;
    const len = 4 + rnd(seed + i) * 3;
    g.moveTo(p.x - 1, p.y - 14)
      .lineTo(p.x - 1 + Math.cos(angle) * len, p.y - 14 - Math.sin(angle) * len * 0.4)
      .stroke({ width: 0.8, color: S(MAT.thatchDk, 0.8), alpha: 0.6 });
  }
}

/**
 * farmShed — single-tile low hut: earth-tone walls, dark pitched roof,
 * tiny door notch. Simpler than any real building — NOT mistakable for a
 * codebase building (no roof-color coding, no outline).
 */
export function farmShed(
  g: Graphics,
  proj: Proj,
  gx: number,
  gy: number,
  seed: number,
): void {
  const doorOnRight = rnd(seed + 1) > 0.5;
  const doorOffsetX = doorOnRight ? 3 : -3;
  const doorW = 4 + rnd(seed + 2) * 3; // 4–7 px wide

  const p = proj.p(gx + 0.5, gy + 0.5, 0);
  // Shadow.
  g.ellipse(p.x + 4, p.y + 2, 16, 7).fill({ color: S(MAT.shadow, 0.8), alpha: 0.2 });

  // Walls — a low box (simpler than building kit).
  const wallW = 18;
  const wallH = 12;
  // Right face (lit).
  g.poly([
    p.x, p.y,
    p.x + wallW * 0.5, p.y - wallH * 0.4,
    p.x + wallW * 0.5, p.y - wallH * 0.4 - wallH,
    p.x, p.y - wallH,
  ]).fill({ color: S(MAT.earth, 1.0) });
  // Left face (shadowed).
  g.poly([
    p.x, p.y,
    p.x - wallW * 0.5, p.y - wallH * 0.4,
    p.x - wallW * 0.5, p.y - wallH * 0.4 - wallH,
    p.x, p.y - wallH,
  ]).fill({ color: S(MAT.earth, 0.82) });
  // Top face.
  g.poly([
    p.x, p.y - wallH - wallH * 0.4,
    p.x + wallW * 0.5, p.y - wallH * 0.4 - wallH,
    p.x, p.y - wallH,
    p.x - wallW * 0.5, p.y - wallH * 0.4 - wallH,
  ]).fill({ color: S(MAT.mudDk, 0.95) });

  // Pitched roof — two quads.
  const roofPeak = wallH + 10;
  // Left roof slope.
  g.poly([
    p.x - wallW * 0.5 - 2, p.y - wallH * 0.4 - wallH + 1,
    p.x, p.y - roofPeak,
    p.x, p.y - wallH - wallH * 0.4,
    p.x - wallW * 0.5, p.y - wallH * 0.4 - wallH,
  ]).fill({ color: S(MAT.thatchDk, 0.9) });
  // Right roof slope.
  g.poly([
    p.x + wallW * 0.5 + 2, p.y - wallH * 0.4 - wallH + 1,
    p.x, p.y - roofPeak,
    p.x, p.y - wallH - wallH * 0.4,
    p.x + wallW * 0.5, p.y - wallH * 0.4 - wallH,
  ]).fill({ color: S(MAT.thatch, 0.95) });

  // Door notch — a small dark rectangle on the front face, offset by seed.
  g.rect(p.x + doorOffsetX - doorW / 2, p.y - 8, doorW, 8).fill({ color: S(MAT.woodDk, 0.85) });
}
