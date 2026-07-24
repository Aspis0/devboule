/* =========================================================================
   iso.ts — "Claude Design" isometric foundation (faithful port of iso.js)
   -------------------------------------------------------------------------
   Projection 2:1, tile 96x48, anchor front-bottom, sun top-left.
   v2 adds: warm Mediterranean palette, textured faces (plaster / ashlar /
   marble), tiled terracotta roofs with courses + ridge + antefixes, fluted
   columns, textured ground. Public signatures unchanged from the source so
   the building generators keep working — they just render richer.

   PORTED 1:1 from Polis-handoff/polis/project/js/iso.js (PixiJS v7) to
   PixiJS v8 + ESM/TS. Math/colors/texture loops/antefixes/courses/flutes are
   kept EXACTLY; only the Graphics API calls are translated:
     v7 g.beginFill(c,a); g.drawPolygon(flat); g.endFill();
       → v8 g.poly(flat).fill({ color, alpha });
     v7 g.lineStyle({...}); g.moveTo; g.lineTo; g.lineStyle(0);
       → v8 g.moveTo(...).lineTo(...).stroke({...});
     v7 g.drawRect/drawCircle/drawEllipse + beginFill/endFill
       → v8 g.rect/circle/ellipse(...).fill(...).
   ========================================================================= */

import { Graphics, type FillInput } from "pixi.js";
import { texFillStyle, type SpriteBank } from "../spriteAssets";

export const TILE_W = 96;
export const TILE_H = 48;
const HALF_W = TILE_W / 2;
const HALF_H = TILE_H / 2;
export const Z_UNIT = 56;

export interface Pt {
  x: number;
  y: number;
}

/** Mutable sun direction ('NW' default; matches the renderer's top-left sun). */
export const SUN: { dir: "NW" | "NE" } = { dir: "NW" };

const F: Record<string, number> = {
  top: 1.17,
  left: 0.9,
  right: 0.68,
  slopeL: 1.06,
  slopeR: 0.8,
  gableLit: 1.0,
  gableShade: 0.76,
};

export function faceFactor(face: string): number {
  if (SUN.dir === "NE") {
    const m: Record<string, string> = {
      left: "right",
      right: "left",
      slopeL: "slopeR",
      slopeR: "slopeL",
      gableLit: "gableShade",
      gableShade: "gableLit",
    };
    if (m[face]) return F[m[face]];
  }
  return F[face] !== undefined ? F[face] : 1;
}

export function shade(hex: number, f: number): number {
  let r = (hex >> 16) & 0xff;
  let g = (hex >> 8) & 0xff;
  let b = hex & 0xff;
  // warm bias when darkening (Mediterranean sun)
  if (f < 1) {
    r *= 1 + (1 - f) * 0.06;
  }
  r = Math.min(255, Math.round(r * f));
  g = Math.min(255, Math.round(g * f));
  b = Math.min(255, Math.round(b * f));
  return (r << 16) | (g << 8) | b;
}

export function mix(a: number, b: number, t: number): number {
  const ar = (a >> 16) & 0xff;
  const ag = (a >> 8) & 0xff;
  const ab = a & 0xff;
  const br = (b >> 16) & 0xff;
  const bg = (b >> 8) & 0xff;
  const bb = b & 0xff;
  return (
    (Math.round(ar + (br - ar) * t) << 16) |
    (Math.round(ag + (bg - ag) * t) << 8) |
    Math.round(ab + (bb - ab) * t)
  );
}

export const lerp = (a: Pt, b: Pt, t: number): Pt => ({
  x: a.x + (b.x - a.x) * t,
  y: a.y + (b.y - a.y) * t,
});

// ---- warm palette --------------------------------------------------------
export const MAT = {
  marble: 0xece3cc,
  marbleWarm: 0xe2d4b4,
  marbleCool: 0xe7e2d2,
  stone: 0xcdba8e,
  plinth: 0xc0aa78,
  plinthDk: 0xa88e5e,
  terracotta: 0xc15a33,
  terraTile: 0xce6b40,
  terraDark: 0x95401f,
  terraGrout: 0x86381b,
  wood: 0x6f4a2a,
  woodLight: 0x8c6234,
  woodDk: 0x533619,
  thatch: 0xc2a258,
  thatchDk: 0x9a7c3c,
  mud: 0xc8a06c,
  mudDk: 0xa9824f,
  plaster: 0xe3cfa4,
  plasterDk: 0xc9b083,
  gold: 0xcea53c,
  bronze: 0x9c7b3a,
  copper: 0x6fa890,
  red: 0xa8392e,
  redDeep: 0x84281f,
  blue: 0x35608a,
  blueDeep: 0x274b6e,
  ochre: 0xc98a2b,
  water: 0x3c7b92,
  waterDeep: 0x2e6072,
  leaf: 0x5e7e38,
  leafDk: 0x415c28,
  leafLt: 0x7c9a4c,
  cypress: 0x3e5a30,
  cypressDk: 0x2e4523,
  bush: 0x6c8c40,
  grass: 0x95a85a,
  grassDk: 0x768843,
  earth: 0xb1925f,
  earthDk: 0x927444,
  sand: 0xcdb888,
  ground: 0xb59b68,
  groundEdge: 0x8a6f45,
  flowerA: 0xd46a5a,
  flowerB: 0xe0c04a,
  flowerC: 0xcfcfe0,
  ink: 0x2b2a26,
  shadow: 0x4a3a24,
} as const;

// ---- A5a: real material textures -----------------------------------------
// The kit stays fully procedural (geometry, tier growth, sun shading) — the
// only change is the BASE FILL of faces/roofs/ground: a light SBS texture
// multiplied by the exact same shaded color the flat fill used. Bank absent
// (or a key missing) ⇒ the flat fill below runs unchanged. Buildings are
// baked ONCE per purpose:level:salt (BuildingTextureAtlas), so texture fills cost
// nothing per frame; the bank must be set BEFORE the first bake (the renderer
// constructor does it).
let kitBank: SpriteBank | null = null;

export function setKitSpriteBank(bank: SpriteBank | null): void {
  kitBank = bank;
}

const TEX_KEY: Record<string, string> = {
  ashlar: "tex:ashlar",
  stone: "tex:ashlar",
  plaster: "tex:plaster",
  // Warm mottled variant for the warm-toned walls (marbleWarm/mud). Only a
  // TEXTURE pick — the procedural texFace fallback keeps kind "plaster".
  plasterwarm: "tex:plasterwarm",
  marble: "tex:marble",
  wood: "tex:wood",
  rooftile: "tex:rooftile",
  thatch: "tex:thatch",
  paved: "tex:cobble",
  earth: "tex:dirtolive",
};

// 256px sources: walls at 0.3 ⇒ ~19px ashlar course / ~3 courses per floor
// (Z_UNIT=56); roofs at 0.45 ⇒ ~17px barrel-tile columns. Caesar-ish density.
const WALL_TEX_SCALE = 0.3;
const ROOF_TEX_SCALE = 0.45;

/** Textured fill for a kit material kind, or null ⇒ caller keeps flat color. */
function kitFill(
  kind: string,
  tint: number,
  alpha = 1,
  scale = WALL_TEX_SCALE,
): ReturnType<typeof texFillStyle> {
  const key = TEX_KEY[kind];
  if (!key || !kitBank) return null;
  return texFillStyle(kitBank, key, tint, alpha, scale);
}

/** Material kind for the MAT colors the generators pass as raw colors
 *  (pediments, cylinders, steps) where no explicit `tex` kind exists. */
function kindForMat(col: number): string | null {
  switch (col) {
    case MAT.marble:
    case MAT.marbleCool:
      return "marble";
    case MAT.marbleWarm:
    case MAT.mud:
      return "plasterwarm";
    case MAT.plaster:
    case MAT.plasterDk:
      return "plaster";
    case MAT.stone:
    case MAT.plinth:
    case MAT.plinthDk:
      return "ashlar";
    case MAT.wood:
    case MAT.woodLight:
    case MAT.woodDk:
      return "wood";
    default:
      return null;
  }
}

function polyFill(g: Graphics, pts: Pt[], style: FillInput): void {
  const flat: number[] = [];
  for (const p of pts) flat.push(p.x, p.y);
  g.poly(flat).fill(style);
}

/** Roof-slope texture fill: barrel tiles (or straw for thatch mats), with the
 *  source's courses (image-X) rotated to run parallel to the eave a→b. */
function roofFillFor(
  mat: number,
  lit: number,
  eaveA: Pt,
  eaveB: Pt,
): ReturnType<typeof texFillStyle> {
  const f = kitFill(
    mat === MAT.thatch || mat === MAT.thatchDk ? "thatch" : "rooftile",
    shade(mat, lit),
    1,
    ROOF_TEX_SCALE,
  );
  if (f) f.matrix.rotate(Math.atan2(eaveB.y - eaveA.y, eaveB.x - eaveA.x));
  return f;
}

export interface Proj {
  W: number;
  D: number;
  p(gx: number, gy: number, gz?: number): Pt;
}

export function makeProj(W: number, D: number): Proj {
  const ax = (W - D) * HALF_W;
  const ay = (W + D) * HALF_H;
  return {
    W,
    D,
    p(gx: number, gy: number, gz?: number): Pt {
      return {
        x: (gx - gy) * HALF_W - ax,
        y: (gx + gy) * HALF_H - ay - (gz || 0) * Z_UNIT,
      };
    },
  };
}

export function poly(g: Graphics, pts: Pt[], color: number, alpha?: number): void {
  const flat: number[] = [];
  for (const p of pts) flat.push(p.x, p.y);
  g.poly(flat).fill({ color, alpha: alpha === undefined ? 1 : alpha });
}

export function outlinePoly(
  g: Graphics,
  pts: Pt[],
  color: number,
  width?: number,
  alpha?: number,
): void {
  g.moveTo(pts[0].x, pts[0].y);
  for (let i = 1; i < pts.length; i++) g.lineTo(pts[i].x, pts[i].y);
  g.lineTo(pts[0].x, pts[0].y);
  g.stroke({
    width: width || 1,
    color,
    alpha: alpha === undefined ? 1 : alpha,
    join: "round",
  });
}

export function line(
  g: Graphics,
  a: Pt,
  b: Pt,
  color: number,
  width?: number,
  alpha?: number,
): void {
  g.moveTo(a.x, a.y).lineTo(b.x, b.y).stroke({
    width: width || 1,
    color,
    alpha: alpha === undefined ? 1 : alpha,
    cap: "round",
  });
}

export function panelLeft(
  g: Graphics,
  proj: Proj,
  gx: number,
  gy: number,
  z0: number,
  w: number,
  h: number,
  color: number,
  alpha?: number,
): void {
  poly(
    g,
    [
      proj.p(gx, gy, z0),
      proj.p(gx + w, gy, z0),
      proj.p(gx + w, gy, z0 + h),
      proj.p(gx, gy, z0 + h),
    ],
    color,
    alpha,
  );
}

export function panelRight(
  g: Graphics,
  proj: Proj,
  gx: number,
  gy: number,
  z0: number,
  d: number,
  h: number,
  color: number,
  alpha?: number,
): void {
  poly(
    g,
    [
      proj.p(gx, gy, z0),
      proj.p(gx, gy + d, z0),
      proj.p(gx, gy + d, z0 + h),
      proj.p(gx, gy, z0 + h),
    ],
    color,
    alpha,
  );
}

// ---- face texturing ------------------------------------------------------
// quad order: [bl, br, tr, tl]  (bottom-left, bottom-right, top-right, top-left)
export function texFace(
  g: Graphics,
  quad: Pt[],
  kind: string,
  base: number,
  lit: number,
): void {
  const [bl, br, tr, tl] = quad;
  const dk = shade(base, lit * 0.8);
  const dk2 = shade(base, lit * 0.66);
  const hl = shade(base, lit * 1.08);
  const hpx = Math.hypot(tl.x - bl.x, tl.y - bl.y); // wall height in px
  if (kind === "ashlar" || kind === "stone") {
    const rows = Math.max(2, Math.round(hpx / 11));
    for (let i = 1; i < rows; i++) {
      const t = i / rows;
      line(g, lerp(bl, tl, t), lerp(br, tr, t), dk, 1, 0.5);
    }
    for (let i = 0; i < rows; i++) {
      const t0 = i / rows;
      const t1 = (i + 1) / rows;
      const off = (i % 2) * 0.5;
      for (let u = off ? 0.5 : 1; u < 3; u++) {
        const uu = (u + off) / 3;
        if (uu >= 1) continue;
        line(
          g,
          lerp(lerp(bl, br, uu), lerp(tl, tr, uu), t0),
          lerp(lerp(bl, br, uu), lerp(tl, tr, uu), t1),
          dk,
          1,
          0.4,
        );
      }
    }
  } else if (kind === "plaster") {
    // darker base course + warm streaks + top highlight
    poly(g, [bl, br, lerp(br, tr, 0.14), lerp(bl, tl, 0.14)], dk, 0.55);
    line(g, lerp(bl, tl, 0.97), lerp(br, tr, 0.97), hl, 1.4, 0.5);
    for (let i = 0; i < 3; i++) {
      const u = 0.25 + i * 0.25;
      line(
        g,
        lerp(lerp(bl, br, u), lerp(tl, tr, u), 0.18),
        lerp(lerp(bl, br, u), lerp(tl, tr, u), 0.92),
        dk,
        1,
        0.12,
      );
    }
  } else if (kind === "marble") {
    line(g, lerp(bl, tl, 0.5), lerp(br, tr, 0.5), hl, 1, 0.3);
    poly(g, [bl, br, lerp(br, tr, 0.08), lerp(bl, tl, 0.08)], dk, 0.4);
    for (let i = 0; i < 2; i++) {
      const u = 0.35 + i * 0.3;
      line(
        g,
        lerp(lerp(bl, br, u), lerp(tl, tr, u), 0.1),
        lerp(lerp(bl, br, u), lerp(tl, tr, u), 0.95),
        dk2,
        1,
        0.1,
      );
    }
  } else if (kind === "wood") {
    const rows = Math.max(2, Math.round(hpx / 9));
    for (let i = 1; i < rows; i++)
      line(g, lerp(bl, tl, i / rows), lerp(br, tr, i / rows), dk, 1, 0.3);
  }
}

// ---- ground plot (textured) ----------------------------------------------
export interface GroundOpt {
  color?: number;
  paved?: boolean;
  grid?: boolean;
  edge?: boolean;
}

export function ground(
  g: Graphics,
  proj: Proj,
  W: number,
  D: number,
  opt?: GroundOpt,
): void {
  opt = opt || {};
  const base = opt.color !== undefined ? opt.color : MAT.ground;
  const paved = opt.paved;
  for (let gx = 0; gx < W; gx++) {
    for (let gy = 0; gy < D; gy++) {
      const a = proj.p(gx, gy, 0);
      const b = proj.p(gx + 1, gy, 0);
      const c = proj.p(gx + 1, gy + 1, 0);
      const d = proj.p(gx, gy + 1, 0);
      const v = ((gx * 7 + gy * 13) % 5) / 5;
      const col = paved
        ? shade(base, 0.97 + v * 0.06)
        : shade(
            mix(base, (gx + gy) % 2 ? MAT.grassDk : MAT.earth, 0.18 + v * 0.12),
            1,
          );
      // A5a — textured plot ground: cobbles when paved, packed earth
      // otherwise (matches the terrain textures around the building).
      const tileFill = kitFill(paved ? "paved" : "earth", col, 1, 0.35);
      if (tileFill) polyFill(g, [a, b, c, d], tileFill);
      else poly(g, [a, b, c, d], col);
      if (opt.grid)
        outlinePoly(
          g,
          [a, b, c, d],
          paved ? shade(base, 0.8) : MAT.groundEdge,
          1,
          paved ? 0.5 : 0.25,
        );
      if (!paved && !opt.grid) {
        // grass tufts + pebbles
        const cx = (a.x + c.x) / 2;
        const cy = (a.y + c.y) / 2;
        if (v > 0.55) {
          for (let k = -1; k <= 1; k++) {
            g.moveTo(cx + k * 4 + v * 5, cy + 4).lineTo(
              cx + k * 4 + v * 5 - 1,
              cy - 1,
            );
          }
          g.stroke({ width: 1, color: MAT.grass, alpha: 0.8 });
        } else if (v < 0.2) {
          g.circle(cx - 6 + v * 30, cy, 1.6).fill({
            color: MAT.earthDk,
            alpha: 0.5,
          });
        }
      }
    }
  }
  if (opt.edge) {
    const a = proj.p(0, 0, 0);
    const b = proj.p(W, 0, 0);
    const c = proj.p(W, D, 0);
    const d = proj.p(0, D, 0);
    outlinePoly(g, [a, b, c, d], MAT.groundEdge, 1.5, 0.7);
  }
}

// ---- box (with optional textured faces) ----------------------------------
export interface BoxOpt {
  topColor?: number;
  leftColor?: number;
  rightColor?: number;
  top?: boolean;
  tex?: string;
  outline?: boolean;
  outlineW?: number;
  outlineColor?: number;
  outlineAlpha?: number;
}

export interface BoxResult {
  T: Pt[];
  L: Pt[];
  R: Pt[];
  x1: number;
  y1: number;
  z1: number;
}

export function box(
  g: Graphics,
  proj: Proj,
  x0: number,
  y0: number,
  z0: number,
  w: number,
  d: number,
  h: number,
  baseColor: number,
  opt?: BoxOpt,
): BoxResult {
  opt = opt || {};
  const x1 = x0 + w;
  const y1 = y0 + d;
  const z1 = z0 + h;
  const P = (a: number, b: number, cz: number): Pt => proj.p(a, b, cz);
  const T = [P(x0, y0, z1), P(x1, y0, z1), P(x1, y1, z1), P(x0, y1, z1)];
  const Lq = [P(x0, y1, z0), P(x1, y1, z0), P(x1, y1, z1), P(x0, y1, z1)]; // bl,br,tr,tl
  const Rq = [P(x1, y0, z0), P(x1, y1, z0), P(x1, y1, z1), P(x1, y0, z1)];
  const cTop =
    opt.topColor !== undefined ? opt.topColor : shade(baseColor, faceFactor("top"));
  const cL = shade(
    opt.leftColor !== undefined ? opt.leftColor : baseColor,
    faceFactor("left"),
  );
  const cR = shade(
    opt.rightColor !== undefined ? opt.rightColor : baseColor,
    faceFactor("right"),
  );
  // A5a — textured walls: same shaded colors as the flat path, multiplied
  // over a light material texture. When textured, the procedural texFace
  // line-work is SKIPPED (its coursing would double-pattern the texture's).
  // fillL/fillR resolve from the same key, so they are null together.
  // Warm-toned plaster walls (marbleWarm/mud) pick the warm mottled texture;
  // the texFace fallback below still receives the caller's "plaster" kind.
  const texKind =
    opt.tex === "plaster" &&
    (baseColor === MAT.marbleWarm || baseColor === MAT.mud)
      ? "plasterwarm"
      : opt.tex;
  const fillL = texKind ? kitFill(texKind, cL) : null;
  const fillR = texKind ? kitFill(texKind, cR) : null;
  if (fillL) polyFill(g, Lq, fillL);
  else poly(g, Lq, cL);
  if (fillR) polyFill(g, Rq, fillR);
  else poly(g, Rq, cR);
  if (opt.tex && !fillL) {
    texFace(
      g,
      Lq,
      opt.tex,
      opt.leftColor !== undefined ? opt.leftColor : baseColor,
      faceFactor("left"),
    );
    texFace(
      g,
      Rq,
      opt.tex,
      opt.rightColor !== undefined ? opt.rightColor : baseColor,
      faceFactor("right"),
    );
  }
  if (opt.top !== false) poly(g, T, cTop);
  if (opt.outline) {
    const ow = opt.outlineW || 1;
    const oc = opt.outlineColor || MAT.ink;
    const oa = opt.outlineAlpha || 0.3;
    outlinePoly(g, Lq, oc, ow, oa);
    outlinePoly(g, Rq, oc, ow, oa);
    if (opt.top !== false) outlinePoly(g, T, oc, ow, oa);
  }
  return { T, L: Lq, R: Rq, x1, y1, z1 };
}

export function steps(
  g: Graphics,
  proj: Proj,
  x0: number,
  y0: number,
  z0: number,
  w: number,
  d: number,
  n: number,
  stepH: number,
  inset: number,
  mat?: number,
): number {
  mat = mat || MAT.stone;
  let zx = z0;
  for (let i = 0; i < n; i++) {
    const ins = inset * i;
    box(g, proj, x0 + ins, y0 + ins, zx, w - 2 * ins, d - 2 * ins, stepH, mat, {
      outline: true,
      outlineAlpha: 0.16,
    });
    zx += stepH;
  }
  return zx;
}

// ---- fluted column -------------------------------------------------------
export interface ColumnOpt {
  capH?: number;
  baseH?: number;
  ionic?: boolean;
  /**
   * Accepted for source parity (the building/monument generators pass it through
   * to `colonnade`/`column`), but the column renderer does not draw a per-column
   * outline — the source v7 `column` ignored it too. Kept so callers can forward a
   * single shared opts object. `rot` is likewise accepted for ring-column callers.
   */
  outline?: boolean;
  rot?: number;
}

export function column(
  g: Graphics,
  proj: Proj,
  cx: number,
  cy: number,
  z0: number,
  h: number,
  rad: number,
  mat?: number,
  opt?: ColumnOpt,
): void {
  opt = opt || {};
  mat = mat || MAT.marble;
  const base = proj.p(cx, cy, z0);
  const top = proj.p(cx, cy, z0 + h);
  const wpx = rad * TILE_W;
  const half = wpx / 2;
  const capH = opt.capH !== undefined ? opt.capH : wpx * 0.5;
  const baseH = opt.baseH !== undefined ? opt.baseH : wpx * 0.34;
  const yTop = top.y + capH * 0.3;
  const yBot = base.y - baseH * 0.3;
  const cLit = shade(mat, 1.13);
  const cMid = shade(mat, 0.97);
  const cDk = shade(mat, 0.76);
  // shaft strips
  g.rect(base.x - half, yTop, half * 0.92, yBot - yTop).fill({ color: cLit });
  g.rect(base.x - half * 0.08, yTop, half * 0.5, yBot - yTop).fill({ color: cMid });
  g.rect(base.x + half * 0.42, yTop, half * 0.58, yBot - yTop).fill({ color: cDk });
  // flutes
  for (let i = -2; i <= 2; i++) {
    const fx = base.x + i * half * 0.34;
    g.moveTo(fx, yTop + 2).lineTo(fx, yBot - 2);
  }
  g.stroke({ width: 1, color: cDk, alpha: 0.45 });
  // capital: echinus + abacus
  const capW = half * 1.4;
  g.rect(top.x - capW, yTop - capH * 0.5, capW * 2, capH * 0.5).fill({
    color: shade(mat, 1.06),
  });
  g.rect(top.x - capW * 1.12, yTop - capH, capW * 2.24, capH * 0.5).fill({
    color: shade(mat, 1.1),
  });
  g.rect(top.x - capW * 1.12, yTop - capH * 0.56, capW * 2.24, capH * 0.12).fill({
    color: shade(mat, 0.86),
  });
  if (opt.ionic) {
    g.circle(top.x - capW * 0.7, yTop - capH * 0.72, capH * 0.3).fill({
      color: shade(mat, 0.82),
    });
    g.circle(top.x + capW * 0.7, yTop - capH * 0.72, capH * 0.3).fill({
      color: shade(mat, 0.82),
    });
    g.circle(top.x - capW * 0.7, yTop - capH * 0.72, capH * 0.13).fill({
      color: shade(mat, 1.12),
    });
    g.circle(top.x + capW * 0.7, yTop - capH * 0.72, capH * 0.13).fill({
      color: shade(mat, 1.12),
    });
  }
  // base
  g.rect(base.x - half * 1.22, yBot, half * 2.44, baseH).fill({
    color: shade(mat, 1.05),
  });
  g.rect(base.x - half * 1.22, yBot + baseH * 0.62, half * 2.44, baseH * 0.38).fill({
    color: shade(mat, 0.8),
  });
}

export function colonnade(
  g: Graphics,
  proj: Proj,
  gx0: number,
  gy0: number,
  gx1: number,
  gy1: number,
  z0: number,
  h: number,
  rad: number,
  count: number,
  mat?: number,
  opt?: ColumnOpt,
): void {
  const pts: Pt[] = [];
  for (let i = 0; i < count; i++) {
    const t = count === 1 ? 0 : i / (count - 1);
    pts.push({ x: gx0 + (gx1 - gx0) * t, y: gy0 + (gy1 - gy0) * t });
  }
  pts.sort((a, b) => a.x + a.y - (b.x + b.y));
  for (const p of pts) column(g, proj, p.x, p.y, z0, h, rad, mat, opt);
}

// ---- tiled terracotta roof helpers ---------------------------------------
export interface TileOpt {
  antefix?: boolean;
}

export function tileQuad(
  g: Graphics,
  eaveL: Pt,
  eaveR: Pt,
  ridgeL: Pt,
  ridgeR: Pt,
  base: number,
  lit: number,
  opt?: TileOpt,
): void {
  opt = opt || {};
  // A5a — real tile texture: the source's courses run along image-X, so
  // rotate the fill matrix to lay image-X along the EAVE (courses parallel to
  // the eave, barrel columns up the slope). Thatch mats pick the straw
  // texture. Textured ⇒ procedural courses/seams are skipped (the texture IS
  // the coursing); the eave gutter + antefixes below always draw.
  const roofFill = roofFillFor(base, lit, eaveL, eaveR);
  if (roofFill) {
    polyFill(g, [eaveL, eaveR, ridgeR, ridgeL], roofFill);
  } else {
    poly(g, [eaveL, eaveR, ridgeR, ridgeL], shade(base, lit));
    const span = Math.hypot(ridgeL.x - eaveL.x, ridgeL.y - eaveL.y);
    const rows = Math.max(2, Math.round(span / 8));
    // courses (parallel to eave), slightly lighter toward ridge
    for (let i = 1; i <= rows; i++) {
      const t = i / rows;
      const a = lerp(eaveL, ridgeL, t);
      const b = lerp(eaveR, ridgeR, t);
      line(g, a, b, shade(base, lit * 0.74), 1.3, 0.55);
      if (i < rows) {
        const t2 = (i + 0.5) / rows;
        line(
          g,
          lerp(eaveL, ridgeL, t2),
          lerp(eaveR, ridgeR, t2),
          shade(base, lit * 1.06),
          1,
          0.3,
        );
      }
    }
    // pan seams (up the slope)
    const seams = Math.max(
      3,
      Math.round(Math.hypot(eaveR.x - eaveL.x, eaveR.y - eaveL.y) / 9),
    );
    for (let i = 1; i < seams; i++) {
      const u = i / seams;
      line(
        g,
        lerp(eaveL, eaveR, u),
        lerp(ridgeL, ridgeR, u),
        shade(base, lit * 0.7),
        1,
        0.3,
      );
    }
  }
  // eave gutter + antefixes
  line(g, eaveL, eaveR, shade(base, lit * 0.6), 1.8, 0.7);
  if (opt.antefix !== false) {
    const na = Math.max(
      2,
      Math.round(Math.hypot(eaveR.x - eaveL.x, eaveR.y - eaveL.y) / 16),
    );
    for (let i = 0; i <= na; i++) {
      const p = lerp(eaveL, eaveR, i / na);
      g.circle(p.x, p.y - 1, 1.5).fill({ color: shade(base, lit * 1.12) });
    }
  }
}

export interface GableOpt {
  overhang?: number;
  ridge?: "x" | "y";
  pediment?: number;
  tympanum?: number;
  outline?: boolean;
}

export function gableRoof(
  g: Graphics,
  proj: Proj,
  x0: number,
  y0: number,
  zt: number,
  w: number,
  d: number,
  rh: number,
  mat?: number,
  opt?: GableOpt,
): Pt[] {
  opt = opt || {};
  mat = mat || MAT.terracotta;
  const o = opt.overhang !== undefined ? opt.overhang : 0.14;
  const ridge = opt.ridge || "y";
  const P = (a: number, b: number, cz: number): Pt => proj.p(a, b, cz);
  const pedMat = opt.pediment !== undefined ? opt.pediment : MAT.marble;
  if (ridge === "y") {
    const rx = x0 + w / 2;
    const eaveR1 = P(x0 + w + o, y0 - o, zt);
    const eaveR2 = P(x0 + w + o, y0 + d + o, zt);
    const ridgeF = P(rx, y0 + d + o, zt + rh);
    const ridgeB = P(rx, y0 - o, zt + rh);
    const eaveL1 = P(x0 - o, y0 + d + o, zt);
    const eaveL2 = P(x0 - o, y0 - o, zt);
    // back-left slope (faint) — A5a: textured like the lit slope, darker tint
    const backLit = faceFactor("slopeL") * 0.9;
    const backFill = roofFillFor(mat, backLit, eaveL1, eaveL2);
    if (backFill) polyFill(g, [eaveL1, eaveL2, ridgeB, ridgeF], backFill);
    else poly(g, [eaveL1, eaveL2, ridgeB, ridgeF], shade(mat, backLit));
    // right slope (visible, tiled)
    tileQuad(g, eaveR2, eaveR1, ridgeF, ridgeB, mat, faceFactor("slopeR"));
    // ridge cap
    line(g, ridgeB, ridgeF, shade(mat, faceFactor("slopeR") * 1.1), 2.4, 0.9);
    // front pediment
    const triL = P(x0 - o, y0 + d + o, zt);
    const triR = P(x0 + w + o, y0 + d + o, zt);
    const triTop = P(rx, y0 + d + o, zt + rh);
    // A5a — textured pediment (material inferred from the MAT color).
    const pedKind = kindForMat(pedMat);
    const pedCol = shade(pedMat, faceFactor("gableLit"));
    const pedFill = pedKind ? kitFill(pedKind, pedCol) : null;
    if (pedFill) polyFill(g, [triL, triR, triTop], pedFill);
    else poly(g, [triL, triR, triTop], pedCol);
    if (opt.tympanum)
      poly(
        g,
        [
          P(x0 + w * 0.14, y0 + d + o, zt + rh * 0.1),
          P(x0 + w * 0.86, y0 + d + o, zt + rh * 0.1),
          P(rx, y0 + d + o, zt + rh * 0.82),
        ],
        shade(opt.tympanum, faceFactor("gableLit")),
      );
    // raking cornice + dentils
    line(g, triL, triTop, shade(pedMat, 0.62), 2, 0.6);
    line(g, triR, triTop, shade(pedMat, 0.62), 2, 0.6);
    line(g, triL, triR, shade(pedMat, 0.7), 2, 0.5);
    const dn = Math.max(3, Math.round(w * 3));
    for (let i = 1; i < dn; i++) {
      const p = lerp(triL, triR, i / dn);
      g.rect(p.x - 1, p.y - 4, 2, 3).fill({ color: shade(pedMat, 0.66), alpha: 0.5 });
    }
    return [triL, triR, triTop];
  } else {
    const ry = y0 + d / 2;
    const eaveF1 = P(x0 - o, y0 + d + o, zt);
    const eaveF2 = P(x0 + w + o, y0 + d + o, zt);
    const ridgeR = P(x0 + w + o, ry, zt + rh);
    const ridgeL = P(x0 - o, ry, zt + rh);
    tileQuad(g, eaveF1, eaveF2, ridgeL, ridgeR, mat, faceFactor("slopeL"));
    line(g, ridgeL, ridgeR, shade(mat, faceFactor("slopeL") * 1.08), 2.4, 0.9);
    const triF = P(x0 + w + o, y0 + d + o, zt);
    const triB = P(x0 + w + o, y0 - o, zt);
    const triTop = P(x0 + w + o, ry, zt + rh);
    const pedKind = kindForMat(pedMat);
    const pedCol = shade(pedMat, faceFactor("gableShade"));
    const pedFill = pedKind ? kitFill(pedKind, pedCol) : null;
    if (pedFill) polyFill(g, [triF, triB, triTop], pedFill);
    else poly(g, [triF, triB, triTop], pedCol);
    if (opt.tympanum)
      poly(
        g,
        [
          P(x0 + w + o, y0 + d * 0.14, zt + rh * 0.1),
          P(x0 + w + o, y0 + d * 0.86, zt + rh * 0.1),
          P(x0 + w + o, ry, zt + rh * 0.82),
        ],
        shade(opt.tympanum, faceFactor("gableShade")),
      );
    line(g, triF, triTop, shade(pedMat, 0.55), 2, 0.6);
    line(g, triB, triTop, shade(pedMat, 0.55), 2, 0.6);
    return [triF, triB, triTop];
  }
}

export interface HipOpt {
  overhang?: number;
  outline?: boolean;
}

// hipped tiled roof (rectangular → ridge; square → pyramid)
export function hipRoof(
  g: Graphics,
  proj: Proj,
  x0: number,
  y0: number,
  zt: number,
  w: number,
  d: number,
  rh: number,
  mat?: number,
  opt?: HipOpt,
): Pt {
  opt = opt || {};
  mat = mat || MAT.terracotta;
  const o = opt.overhang !== undefined ? opt.overhang : 0.12;
  const P = (a: number, b: number, cz: number): Pt => proj.p(a, b, cz);
  const inset = Math.min(w, d) * 0.28;
  const rL = P(x0 + inset, y0 + d / 2, zt + rh);
  const rR = P(x0 + w - inset, y0 + d / 2, zt + rh);
  const e = {
    ne: P(x0 - o, y0 - o, zt),
    nw: P(x0 + w + o, y0 - o, zt),
    sw: P(x0 + w + o, y0 + d + o, zt),
    se: P(x0 - o, y0 + d + o, zt),
  };
  // front (gy+) trapezoid — lit, tiled
  tileQuad(g, e.se, e.sw, rL, rR, mat, faceFactor("slopeL"));
  // right (gx+) trapezoid
  tileQuad(g, e.sw, e.nw, rR, rR, mat, faceFactor("slopeR"));
  // ridge + hips
  line(g, rL, rR, shade(mat, faceFactor("slopeL") * 1.1), 2.4, 0.9);
  line(g, e.se, rL, shade(mat, 0.6), 1.4, 0.6);
  line(g, e.sw, rL, shade(mat, 0.6), 1.4, 0.6);
  line(g, e.sw, rR, shade(mat, 0.6), 1.4, 0.6);
  return P(x0 + w / 2, y0 + d / 2, zt + rh);
}

export interface CylinderOpt {
  seg?: number;
  outline?: boolean;
}

export function cylinder(
  g: Graphics,
  proj: Proj,
  cx: number,
  cy: number,
  z0: number,
  rad: number,
  h: number,
  mat?: number,
  opt?: CylinderOpt,
): number {
  opt = opt || {};
  mat = mat || MAT.marble;
  const seg = opt.seg || 16;
  const top: Pt[] = [];
  const bot: Pt[] = [];
  for (let i = 0; i <= seg; i++) {
    const a = (i / seg) * Math.PI * 2;
    const gx = cx + Math.cos(a) * rad;
    const gy = cy + Math.sin(a) * rad;
    top.push(proj.p(gx, gy, z0 + h));
    bot.push(proj.p(gx, gy, z0));
  }
  // A5a — textured shaft: per-segment tint keeps the roundness gradient; the
  // texture is continuous across segments (fills share the Graphics' space).
  const cylKind = kindForMat(mat);
  for (let i = 0; i < seg; i++) {
    const ang = ((i + 0.5) / seg) * Math.PI * 2;
    if (Math.sin(ang) <= -0.25) continue;
    const lr = Math.cos(ang);
    const f = 0.95 + lr * (SUN.dir === "NE" ? 0.24 : -0.24);
    const quad = [bot[i], bot[i + 1], top[i + 1], top[i]];
    const segCol = shade(mat, Math.max(0.58, f));
    const segFill = cylKind ? kitFill(cylKind, segCol) : null;
    if (segFill) polyFill(g, quad, segFill);
    else poly(g, quad, segCol);
  }
  // courses
  for (let r = 1; r < Math.max(2, Math.round((h * Z_UNIT) / 12)); r++) {
    const t = r / Math.max(2, Math.round((h * Z_UNIT) / 12));
    for (let i = 0; i < seg; i++) {
      const ang = ((i + 0.5) / seg) * Math.PI * 2;
      if (Math.sin(ang) <= -0.25) continue;
      const a = lerp(bot[i], top[i], t);
      const b = lerp(bot[i + 1], top[i + 1], t);
      g.moveTo(a.x, a.y).lineTo(b.x, b.y);
    }
    g.stroke({ width: 1, color: shade(mat, 0.7), alpha: 0.3 });
  }
  poly(g, top, shade(mat, faceFactor("top")));
  if (opt.outline) outlinePoly(g, top, MAT.ink, 1, 0.22);
  return z0 + h;
}

export function project(proj: Proj, gx: number, gy: number, gz?: number): Pt {
  return proj.p(gx, gy, gz);
}
