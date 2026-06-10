/* =========================================================================
   figures.ts — Monumental sculpted figures (faithful port of figures.js)
   -------------------------------------------------------------------------
   Screen-space "billboard" figures drawn at a base point (the feet/plinth
   top), extending upward (−y). Same primitive language as PROP.statue but
   bigger and characterful: heroic kouros (Colossus), enthroned deity
   (Zeus), standing goddess (Athena Parthenos), caryatid maiden, quadriga,
   winged Victory. Materials follow ISO.MAT; a left-lit (NW sun) read is
   baked in (lit on the figure's right side = viewer left).
   Each fn(g, x, y, sc, opt) draws into Graphics g. Heights ≈ 90·sc px.

   PORTED 1:1 from Polis-handoff/polis/project/js/figures.js (PixiJS v7) →
   PixiJS v8 / ESM-TS. The v7 immediate-mode calls are translated EXACTLY the
   same way the rest of the kitcd port does it:
     v7 g.beginFill(c,a); g.drawPolygon([...]); g.endFill();
       → v8 g.poly([...]).fill({ color, alpha });
     v7 g.beginFill(c,a); g.drawRect/drawCircle/drawEllipse(...); g.endFill();
       → v8 g.rect/circle/ellipse(...).fill({ color, alpha });
     v7 g.lineStyle({...}); g.moveTo; g.lineTo / g.arc; g.lineStyle(0);
       → v8 g.moveTo/lineTo/arc(...).stroke({...});
   Geometry / materials / shading factors are kept verbatim. These are STATIC
   billboards (no per-frame state); the animated parts a wonder uses (torch
   beacon, altar flame) live in anims.ts and attach at the points these return.
   ========================================================================= */

import { Graphics } from "pixi.js";
import { MAT, shade as S } from "./iso";

const M = MAT;

interface Tone {
  hl: number;
  lit: number;
  mid: number;
  dk: number;
  dk2: number;
}

function tone(mat: number): Tone {
  return {
    hl: S(mat, 1.2),
    lit: S(mat, 1.08),
    mid: S(mat, 0.95),
    dk: S(mat, 0.74),
    dk2: S(mat, 0.6),
  };
}

function shadow(g: Graphics, x: number, y: number, sc: number, w?: number): void {
  g.ellipse(x + 3 * sc, y, (w || 11) * sc, 3.4 * sc).fill({
    color: M.shadow,
    alpha: 0.18,
  });
}

// a tapered limb/trunk as a 4-pt poly between two widths
function taper(
  g: Graphics,
  x0: number,
  y0: number,
  w0: number,
  x1: number,
  y1: number,
  w1: number,
  col: number,
): void {
  g.poly([x0 - w0, y0, x0 + w0, y0, x1 + w1, y1, x1 - w1, y1]).fill({ color: col });
}

export interface HeroicResult {
  torch: { x: number; y: number };
  head: { x: number; y: number };
}

export interface HeroicOpt {
  mat?: number;
  cloth?: number;
  helios?: boolean;
  torch?: boolean;
}

// ---- heroic standing male (Colossus of Rhodes / Helios) -----------------
export function heroicMale(
  g: Graphics,
  x: number,
  y: number,
  sc: number,
  opt?: HeroicOpt,
): HeroicResult {
  opt = opt || {};
  const mat = opt.mat || M.bronze;
  const t = tone(mat);
  shadow(g, x, y, sc, 13);
  const hipY = y - 38 * sc;
  const shoY = y - 64 * sc;
  const headY = y - 76 * sc;
  // legs (slight stance) — right (viewer-left) lit, left in shade
  taper(g, x - 6 * sc, y, 4.6 * sc, x - 4 * sc, hipY, 4 * sc, t.lit);
  taper(g, x + 6 * sc, y, 4.6 * sc, x + 4 * sc, hipY, 4 * sc, t.dk);
  // shin highlights
  g.rect(x - 9 * sc, hipY + 4 * sc, 2 * sc, y - hipY - 6 * sc).fill({
    color: t.hl,
    alpha: 0.5,
  });
  // hip wrap / short himation
  const cloth = opt.cloth || mat;
  g.poly([
    x - 9 * sc,
    hipY - 2 * sc,
    x + 9 * sc,
    hipY - 2 * sc,
    x + 7 * sc,
    hipY + 9 * sc,
    x - 7 * sc,
    hipY + 9 * sc,
  ]).fill({ color: S(cloth, 0.9) });
  g.poly([
    x - 9 * sc,
    hipY - 2 * sc,
    x - 1 * sc,
    hipY - 2 * sc,
    x - 2 * sc,
    hipY + 9 * sc,
    x - 7 * sc,
    hipY + 9 * sc,
  ]).fill({ color: S(cloth, 1.1) });
  // torso (V-taper to broad shoulders)
  g.poly([
    x - 5 * sc,
    hipY,
    x + 5 * sc,
    hipY,
    x + 11 * sc,
    shoY,
    x - 11 * sc,
    shoY,
  ]).fill({ color: t.mid });
  g.poly([
    x - 5 * sc,
    hipY,
    x - 0.5 * sc,
    hipY,
    x - 1 * sc,
    shoY,
    x - 11 * sc,
    shoY,
  ]).fill({ color: t.lit });
  g.poly([
    x + 2 * sc,
    hipY,
    x + 5 * sc,
    hipY,
    x + 11 * sc,
    shoY,
    x + 5 * sc,
    shoY,
  ]).fill({ color: t.dk, alpha: 0.55 });
  // pectoral / ab hints
  g.moveTo(x - 6 * sc, shoY + 5 * sc)
    .lineTo(x + 6 * sc, shoY + 5 * sc)
    .moveTo(x, shoY + 4 * sc)
    .lineTo(x, hipY - 2 * sc)
    .stroke({ width: 1, color: t.dk, alpha: 0.4 });
  // left arm down at side (viewer right)
  taper(g, x + 10 * sc, shoY + 1 * sc, 3 * sc, x + 12 * sc, hipY + 2 * sc, 2.4 * sc, t.dk);
  // right arm raised holding torch (viewer left)
  const handX = x - 17 * sc;
  const handY = y - 96 * sc;
  taper(g, x - 10 * sc, shoY + 1 * sc, 3.2 * sc, x - 15 * sc, shoY - 16 * sc, 2.8 * sc, t.lit);
  taper(g, x - 15 * sc, shoY - 16 * sc, 2.8 * sc, handX, handY, 2.4 * sc, t.lit);
  // neck + head
  g.rect(x - 2.2 * sc, headY, 4.4 * sc, shoY - headY + 2 * sc).fill({ color: t.mid });
  g.circle(x - 0.6 * sc, headY - 4 * sc, 6 * sc).fill({ color: t.lit });
  g.ellipse(x + 3 * sc, headY - 4 * sc, 2.6 * sc, 6 * sc).fill({
    color: t.dk,
    alpha: 0.5,
  });
  // radiate crown (Helios) — sun rays around the head
  if (opt.helios) {
    for (let i = 0; i < 9; i++) {
      const a = -Math.PI * 0.95 + (i / 8) * Math.PI * 0.9;
      const hx = x - 0.6 * sc + Math.cos(a) * 6.5 * sc;
      const hy = headY - 4 * sc + Math.sin(a) * 6.5 * sc;
      g.moveTo(hx, hy).lineTo(hx + Math.cos(a) * 6 * sc, hy + Math.sin(a) * 6 * sc);
    }
    g.stroke({ width: 1.6 * sc, color: t.hl, alpha: 0.95 });
  }
  // torch cup at the raised hand (flame added separately by caller)
  if (opt.torch) {
    g.rect(handX - 3 * sc, handY - 1 * sc, 6 * sc, 3 * sc).fill({ color: t.dk });
    g.ellipse(handX, handY - 3 * sc, 4 * sc, 2 * sc).fill({ color: t.hl });
  }
  return { torch: { x: handX, y: handY }, head: { x: x, y: headY - 4 * sc } };
}

export interface DeityOpt {
  gold?: number;
  ivory?: number;
}

export interface HeadResult {
  head: { x: number; y: number };
}

// ---- enthroned deity (Zeus at Olympia) — chryselephantine ---------------
export function enthroned(
  g: Graphics,
  x: number,
  y: number,
  sc: number,
  opt?: DeityOpt,
): HeadResult {
  opt = opt || {};
  const gold = opt.gold || M.gold;
  const ivory = opt.ivory || M.marble;
  const tg = tone(gold);
  const ti = tone(ivory);
  shadow(g, x, y, sc, 22);
  // throne block (behind/under)
  const thrTop = y - 30 * sc;
  g.rect(x - 24 * sc, thrTop, 48 * sc, 30 * sc).fill({ color: S(gold, 0.7) }); // seat base
  g.rect(x - 24 * sc, thrTop, 6 * sc, 30 * sc).fill({ color: S(gold, 0.84) });
  g.rect(x + 18 * sc, thrTop, 6 * sc, 30 * sc).fill({ color: S(gold, 0.6) });
  // throne back + finials
  g.rect(x - 24 * sc, y - 78 * sc, 7 * sc, 48 * sc).fill({ color: S(gold, 0.78) });
  g.rect(x + 17 * sc, y - 78 * sc, 7 * sc, 48 * sc).fill({ color: S(gold, 0.78) });
  g.circle(x - 20.5 * sc, y - 80 * sc, 3.4 * sc).fill({ color: tg.hl });
  g.circle(x + 20.5 * sc, y - 80 * sc, 3.4 * sc).fill({ color: tg.hl });
  g.rect(x - 18 * sc, y - 72 * sc, 36 * sc, 42 * sc).fill({ color: S(gold, 0.66) }); // back panel
  g.rect(x - 15 * sc, y - 68 * sc, 30 * sc, 34 * sc).stroke({
    width: 1,
    color: tg.hl,
    alpha: 0.4,
  });
  // lap drapery (himation over legs) — gold
  g.poly([
    x - 18 * sc,
    thrTop - 2 * sc,
    x + 18 * sc,
    thrTop - 2 * sc,
    x + 16 * sc,
    thrTop + 16 * sc,
    x - 16 * sc,
    thrTop + 16 * sc,
  ]).fill({ color: tg.mid });
  g.poly([
    x - 18 * sc,
    thrTop - 2 * sc,
    x - 4 * sc,
    thrTop - 2 * sc,
    x - 5 * sc,
    thrTop + 16 * sc,
    x - 16 * sc,
    thrTop + 16 * sc,
  ]).fill({ color: tg.lit });
  for (let i = -3; i <= 3; i++) {
    g.moveTo(x + i * 5 * sc, thrTop).lineTo(x + i * 5 * sc + 1.5 * sc, thrTop + 15 * sc);
  }
  g.stroke({ width: 1, color: tg.dk, alpha: 0.45 });
  // lower legs / feet on footstool
  g.rect(x - 11 * sc, thrTop + 14 * sc, 7 * sc, 18 * sc).fill({ color: ti.mid });
  g.rect(x + 4 * sc, thrTop + 14 * sc, 7 * sc, 18 * sc).fill({ color: ti.mid });
  g.rect(x - 14 * sc, y - 4 * sc, 28 * sc, 5 * sc).fill({ color: S(gold, 0.8) }); // footstool
  // bare torso (ivory)
  const shoY = y - 60 * sc;
  g.poly([
    x - 9 * sc,
    thrTop,
    x + 9 * sc,
    thrTop,
    x + 12 * sc,
    shoY,
    x - 12 * sc,
    shoY,
  ]).fill({ color: ti.mid });
  g.poly([
    x - 9 * sc,
    thrTop,
    x - 1 * sc,
    thrTop,
    x - 2 * sc,
    shoY,
    x - 12 * sc,
    shoY,
  ]).fill({ color: ti.lit });
  // himation over left shoulder (gold sash)
  g.poly([
    x + 4 * sc,
    shoY - 2 * sc,
    x + 12 * sc,
    shoY,
    x + 8 * sc,
    thrTop,
    x + 2 * sc,
    thrTop,
  ]).fill({ color: tg.mid });
  // arms: right extended holding a small Nike, left raised on a sceptre
  taper(g, x - 11 * sc, shoY + 2 * sc, 3 * sc, x - 22 * sc, shoY + 4 * sc, 2.4 * sc, ti.lit);
  taper(g, x + 11 * sc, shoY + 2 * sc, 3 * sc, x + 19 * sc, shoY - 4 * sc, 2.4 * sc, ti.dk);
  // sceptre (left hand, viewer right)
  g.rect(x + 18 * sc, y - 92 * sc, 2.2 * sc, 88 * sc).fill({ color: S(gold, 0.82) });
  g.circle(x + 19 * sc, y - 92 * sc, 3.6 * sc).fill({ color: tg.hl }); // eagle finial
  // little Nike figurine on right palm (viewer left)
  miniNike(g, x - 24 * sc, shoY + 4 * sc, sc * 0.5, gold);
  // head — bearded, olive wreath
  const headY = y - 70 * sc;
  g.circle(x, headY, 6.5 * sc).fill({ color: ti.lit });
  g.ellipse(x, headY + 4 * sc, 6 * sc, 5 * sc).fill({ color: S(M.wood, 0.95) }); // beard
  g.ellipse(x - 2 * sc, headY - 4 * sc, 7 * sc, 4 * sc).fill({ color: S(M.wood, 0.8) }); // hair
  g.arc(x, headY - 2 * sc, 8 * sc, Math.PI * 1.05, Math.PI * 1.95).stroke({
    width: 1.6 * sc,
    color: M.leafDk,
  }); // wreath
  return { head: { x, y: headY } };
}

// ---- standing goddess (Athena Parthenos) --------------------------------
export function goddess(
  g: Graphics,
  x: number,
  y: number,
  sc: number,
  opt?: DeityOpt,
): HeadResult {
  opt = opt || {};
  const gold = opt.gold || M.gold;
  const ivory = opt.ivory || M.marble;
  const tg = tone(gold);
  const ti = tone(ivory);
  shadow(g, x, y, sc, 13);
  const hemY = y;
  const kneeY = y - 30 * sc;
  const waistY = y - 48 * sc;
  const shoY = y - 64 * sc;
  const headY = y - 76 * sc;
  // peplos skirt (gold, bell drape with vertical folds)
  g.poly([
    x - 13 * sc,
    hemY,
    x + 13 * sc,
    hemY,
    x + 7 * sc,
    waistY,
    x - 7 * sc,
    waistY,
  ]).fill({ color: tg.mid });
  g.poly([
    x - 13 * sc,
    hemY,
    x - 3 * sc,
    hemY,
    x - 3 * sc,
    waistY,
    x - 7 * sc,
    waistY,
  ]).fill({ color: tg.lit });
  g.poly([
    x + 4 * sc,
    hemY,
    x + 13 * sc,
    hemY,
    x + 7 * sc,
    waistY,
    x + 3 * sc,
    waistY,
  ]).fill({ color: tg.dk, alpha: 0.5 });
  for (let i = -3; i <= 3; i++) {
    g.moveTo(x + i * 3.6 * sc, waistY + 2 * sc).lineTo(x + i * 4.6 * sc, hemY - 1 * sc);
  }
  g.stroke({ width: 1, color: tg.dk, alpha: 0.5 });
  g.rect(x - 13 * sc, hemY - 2 * sc, 26 * sc, 2 * sc).fill({ color: tg.hl, alpha: 0.6 }); // hem band
  void kneeY;
  // upper body (peplos, gold) with aegis bib (ivory scales)
  g.poly([
    x - 7 * sc,
    waistY,
    x + 7 * sc,
    waistY,
    x + 9 * sc,
    shoY,
    x - 9 * sc,
    shoY,
  ]).fill({ color: tg.mid });
  g.poly([
    x - 7 * sc,
    waistY,
    x - 1 * sc,
    waistY,
    x - 2 * sc,
    shoY,
    x - 9 * sc,
    shoY,
  ]).fill({ color: tg.lit });
  g.poly([x - 6 * sc, shoY + 1 * sc, x + 6 * sc, shoY + 1 * sc, x, shoY + 9 * sc]).fill({
    color: ti.lit,
  }); // aegis
  // arms: right extended forward holding Nike, left hand resting (shield)
  taper(g, x - 8 * sc, shoY + 2 * sc, 2.6 * sc, x - 20 * sc, shoY + 8 * sc, 2 * sc, ti.lit);
  taper(g, x + 8 * sc, shoY + 2 * sc, 2.6 * sc, x + 12 * sc, waistY, 2 * sc, ti.dk);
  // Nike on the outstretched right palm
  miniNike(g, x - 22 * sc, shoY + 8 * sc, sc * 0.46, gold);
  // big round shield resting at left side (viewer right), + coiled spear
  g.circle(x + 17 * sc, y - 18 * sc, 14 * sc).fill({ color: S(gold, 0.7) });
  g.circle(x + 14 * sc, y - 21 * sc, 12 * sc).fill({ color: tg.lit });
  g.circle(x + 17 * sc, y - 18 * sc, 4.5 * sc).fill({ color: S(gold, 0.62) }); // boss
  g.circle(x + 16 * sc, y - 19 * sc, 8.5 * sc).stroke({
    width: 1,
    color: S(gold, 0.6),
    alpha: 0.5,
  });
  g.rect(x + 22 * sc, y - 92 * sc, 1.8 * sc, 92 * sc).fill({ color: S(M.bronze, 0.9) }); // spear
  // neck + head with high-crested Attic helmet
  g.rect(x - 2.4 * sc, headY, 4.8 * sc, shoY - headY + 2 * sc).fill({ color: ti.mid });
  g.circle(x, headY - 4 * sc, 5.6 * sc).fill({ color: ti.lit });
  g.rect(x - 6 * sc, headY - 12 * sc, 12 * sc, 6 * sc).fill({ color: tg.mid }); // helmet bowl
  g.poly([
    x - 6 * sc,
    headY - 11 * sc,
    x + 6 * sc,
    headY - 11 * sc,
    x + 7 * sc,
    headY - 7 * sc,
    x - 7 * sc,
    headY - 7 * sc,
  ]).fill({ color: tg.dk });
  // crest
  g.poly([
    x - 5 * sc,
    headY - 12 * sc,
    x + 5 * sc,
    headY - 12 * sc,
    x + 7 * sc,
    headY - 22 * sc,
    x - 3 * sc,
    headY - 19 * sc,
  ]).fill({ color: S(M.red, 1.0) });
  g.poly([
    x - 5 * sc,
    headY - 12 * sc,
    x - 1 * sc,
    headY - 12 * sc,
    x + 1 * sc,
    headY - 19 * sc,
    x - 3 * sc,
    headY - 18 * sc,
  ]).fill({ color: S(M.red, 0.7) });
  return { head: { x, y: headY } };
}

// ---- a small winged Victory (held in a deity's hand, or standalone) -----
export function miniNike(
  g: Graphics,
  x: number,
  y: number,
  sc: number,
  mat?: number,
): void {
  mat = mat || M.gold;
  const t = tone(mat);
  g.poly([
    x - 3 * sc,
    y,
    x + 3 * sc,
    y,
    x + 2 * sc,
    y - 10 * sc,
    x - 2 * sc,
    y - 10 * sc,
  ]).fill({ color: t.mid }); // body
  g.circle(x, y - 12 * sc, 2.4 * sc).fill({ color: t.lit }); // head
  // wings
  g.poly([x - 2 * sc, y - 9 * sc, x - 12 * sc, y - 16 * sc, x - 9 * sc, y - 5 * sc]).fill({
    color: t.lit,
    alpha: 0.95,
  });
  g.poly([x + 2 * sc, y - 9 * sc, x + 12 * sc, y - 16 * sc, x + 9 * sc, y - 5 * sc]).fill({
    color: t.dk,
    alpha: 0.9,
  });
}

// ---- caryatid maiden (Erechtheion porch) supporting an entablature ------
export function caryatid(
  g: Graphics,
  x: number,
  y: number,
  sc: number,
  mat?: number,
): void {
  mat = mat || M.marble;
  const t = tone(mat);
  shadow(g, x, y, sc, 9);
  const hemY = y;
  const waistY = y - 30 * sc;
  const shoY = y - 44 * sc;
  const headY = y - 52 * sc;
  // column-like fluted skirt
  g.poly([
    x - 7 * sc,
    hemY,
    x + 7 * sc,
    hemY,
    x + 5 * sc,
    waistY,
    x - 5 * sc,
    waistY,
  ]).fill({ color: t.mid });
  g.poly([
    x - 7 * sc,
    hemY,
    x - 2 * sc,
    hemY,
    x - 2 * sc,
    waistY,
    x - 5 * sc,
    waistY,
  ]).fill({ color: t.lit });
  for (let i = -2; i <= 2; i++) {
    g.moveTo(x + i * 2.6 * sc, waistY + 1 * sc).lineTo(x + i * 3.2 * sc, hemY - 1 * sc);
  }
  g.stroke({ width: 1, color: t.dk, alpha: 0.5 });
  // torso (peplos)
  g.poly([
    x - 5 * sc,
    waistY,
    x + 5 * sc,
    waistY,
    x + 6 * sc,
    shoY,
    x - 6 * sc,
    shoY,
  ]).fill({ color: t.mid });
  g.poly([
    x - 5 * sc,
    waistY,
    x - 1 * sc,
    waistY,
    x - 2 * sc,
    shoY,
    x - 6 * sc,
    shoY,
  ]).fill({ color: t.lit });
  g.rect(x + 3 * sc, shoY, 3 * sc, waistY - shoY).fill({ color: t.dk, alpha: 0.5 });
  // arm hint at side
  g.rect(x + 5 * sc, shoY + 1 * sc, 2.2 * sc, 14 * sc).fill({ color: t.dk });
  // neck + head + capital block above head
  g.rect(x - 2 * sc, headY, 4 * sc, shoY - headY + 1 * sc).fill({ color: t.mid });
  g.circle(x - 0.5 * sc, headY - 2 * sc, 4.2 * sc).fill({ color: t.lit });
  g.ellipse(x + 2.4 * sc, headY - 2 * sc, 1.8 * sc, 4 * sc).fill({ color: t.dk, alpha: 0.4 });
  // hair coils on shoulders
  g.rect(x - 5 * sc, headY, 2 * sc, 8 * sc).fill({ color: t.dk, alpha: 0.5 });
  g.rect(x + 3 * sc, headY, 2 * sc, 8 * sc).fill({ color: t.dk, alpha: 0.5 });
  // capital (echinus + abacus) she carries
  g.rect(x - 6 * sc, headY - 10 * sc, 12 * sc, 4 * sc).fill({ color: t.lit });
  g.rect(x - 4.5 * sc, headY - 6 * sc, 9 * sc, 2 * sc).fill({ color: t.mid });
}

// ---- quadriga (4-horse chariot) atop a monument -------------------------
export function quadriga(
  g: Graphics,
  x: number,
  y: number,
  sc: number,
  mat?: number,
): void {
  mat = mat || M.bronze;
  const t = tone(mat);
  shadow(g, x, y, sc, 22);
  // 4 horses abreast (overlapping silhouettes), facing viewer-left
  for (let i = 3; i >= 0; i--) {
    const hx = x - 6 * sc + i * 6 * sc;
    const lit = i < 2 ? t.dk : t.lit;
    g.ellipse(hx, y - 12 * sc, 9 * sc, 6 * sc).fill({ color: lit }); // body
    g.poly([
      hx - 9 * sc,
      y - 14 * sc,
      hx - 16 * sc,
      y - 22 * sc,
      hx - 13 * sc,
      y - 23 * sc,
      hx - 6 * sc,
      y - 13 * sc,
    ]).fill({ color: lit }); // neck/head
    g.rect(hx - 2 * sc, y - 8 * sc, 2 * sc, 8 * sc).fill({ color: S(mat, i < 2 ? 0.5 : 0.85) });
    g.rect(hx + 3 * sc, y - 8 * sc, 2 * sc, 8 * sc).fill({ color: S(mat, i < 2 ? 0.5 : 0.85) }); // legs
  }
  // chariot car + driver behind
  g.rect(x + 10 * sc, y - 16 * sc, 12 * sc, 12 * sc).fill({ color: S(mat, 0.7) });
  g.circle(x + 12 * sc, y - 6 * sc, 5 * sc).fill({ color: t.lit }); // wheel
  for (let k = 0; k < 4; k++) {
    const a = (k * Math.PI) / 4;
    g.moveTo(x + 12 * sc, y - 6 * sc).lineTo(
      x + 12 * sc + Math.cos(a) * 5 * sc,
      y - 6 * sc + Math.sin(a) * 5 * sc,
    );
  }
  g.stroke({ width: 1, color: t.dk2, alpha: 0.6 });
  g.rect(x + 14 * sc, y - 30 * sc, 5 * sc, 16 * sc).fill({ color: t.mid }); // driver torso
  g.circle(x + 16.5 * sc, y - 32 * sc, 3 * sc).fill({ color: t.lit }); // head
}

export interface WingedOpt {
  mat?: number;
}

// ---- winged Victory on a ship prow (Nike of Samothrace) -----------------
export function wingedVictory(
  g: Graphics,
  x: number,
  y: number,
  sc: number,
  opt?: WingedOpt,
): void {
  opt = opt || {};
  const mat = opt.mat || M.marble;
  const t = tone(mat);
  shadow(g, x, y, sc, 16);
  // ship prow base (angled blocks)
  g.poly([
    x - 18 * sc,
    y,
    x + 16 * sc,
    y - 4 * sc,
    x + 22 * sc,
    y - 12 * sc,
    x - 14 * sc,
    y - 8 * sc,
  ]).fill({ color: S(mat, 0.78) });
  g.poly([
    x - 18 * sc,
    y,
    x - 14 * sc,
    y - 8 * sc,
    x + 22 * sc,
    y - 12 * sc,
    x + 22 * sc,
    y - 6 * sc,
  ]).fill({ color: S(mat, 0.62) });
  for (let i = 0; i < 4; i++) {
    const u = i / 4;
    g.moveTo(x - 14 * sc + u * 34 * sc, y - 8 * sc).lineTo(x - 18 * sc + u * 36 * sc, y);
  }
  g.stroke({ width: 1, color: t.dk, alpha: 0.4 });
  const baseY = y - 12 * sc;
  // wind-blown drapery skirt
  g.poly([
    x - 10 * sc,
    baseY,
    x + 10 * sc,
    baseY - 2 * sc,
    x + 6 * sc,
    baseY - 30 * sc,
    x - 5 * sc,
    baseY - 30 * sc,
  ]).fill({ color: t.mid });
  g.poly([
    x - 10 * sc,
    baseY,
    x - 2 * sc,
    baseY,
    x - 2 * sc,
    baseY - 30 * sc,
    x - 5 * sc,
    baseY - 30 * sc,
  ]).fill({ color: t.lit });
  for (let i = -3; i <= 3; i++) {
    g.moveTo(x + i * 2.6 * sc, baseY - 28 * sc).lineTo(x + i * 3.4 * sc + 3 * sc, baseY - 1 * sc);
  }
  g.stroke({ width: 1, color: t.dk, alpha: 0.5 });
  // torso
  const shoY = baseY - 44 * sc;
  g.poly([
    x - 5 * sc,
    baseY - 30 * sc,
    x + 5 * sc,
    baseY - 30 * sc,
    x + 8 * sc,
    shoY,
    x - 7 * sc,
    shoY,
  ]).fill({ color: t.mid });
  g.poly([
    x - 5 * sc,
    baseY - 30 * sc,
    x - 1 * sc,
    baseY - 30 * sc,
    x - 2 * sc,
    shoY,
    x - 7 * sc,
    shoY,
  ]).fill({ color: t.lit });
  // two great wings sweeping up-back
  const wing = (dx: number, lit: number): void => {
    g.poly([
      x + dx * 5 * sc,
      shoY + 2 * sc,
      x + dx * 30 * sc,
      shoY - 30 * sc,
      x + dx * 26 * sc,
      shoY - 10 * sc,
      x + dx * 18 * sc,
      shoY - 2 * sc,
    ]).fill({ color: lit, alpha: 0.96 });
    for (let i = 1; i < 5; i++) {
      const u = i / 5;
      g.moveTo(x + dx * (5 + u * 13) * sc, shoY + 2 * sc - u * 2 * sc).lineTo(
        x + dx * (5 + u * 25) * sc,
        shoY - u * 30 * sc,
      );
    }
    g.stroke({ width: 1, color: t.dk, alpha: 0.45 });
  };
  wing(1, t.dk);
  wing(-1, t.lit);
  // head (headless icon, but give a hint of neck) — keep subtle nub
  g.rect(x - 2 * sc, shoY - 2 * sc, 4 * sc, 4 * sc).fill({ color: t.mid });
}

/** FIG namespace mirror of the source's `global.FIG`. */
export const FIG = {
  heroicMale,
  enthroned,
  goddess,
  caryatid,
  quadriga,
  wingedVictory,
  miniNike,
};
