/* =========================================================================
   iso.js — Isometric foundation, v2 "Caesar-detail" (PixiJS v7)
   -------------------------------------------------------------------------
   Projection 2:1, tile 96x48, anchor front-bottom, sun top-left.
   v2 adds: warm Mediterranean palette, textured faces (plaster / ashlar /
   marble), tiled terracotta roofs with courses + ridge + antefixes,
   fluted columns, textured ground. Public signatures unchanged so the
   building generators keep working — they just render richer.
   ========================================================================= */
(function (global) {
  'use strict';

  const TILE_W = 96, TILE_H = 48;
  const HALF_W = TILE_W / 2, HALF_H = TILE_H / 2;
  const Z_UNIT = 56;

  const SUN = { dir: 'NW' };
  const F = {
    top: 1.17, left: 0.9, right: 0.68,
    slopeL: 1.06, slopeR: 0.8,
    gableLit: 1.0, gableShade: 0.76
  };
  function faceFactor(face) {
    if (SUN.dir === 'NE') {
      const m = { left: 'right', right: 'left', slopeL: 'slopeR', slopeR: 'slopeL', gableLit: 'gableShade', gableShade: 'gableLit' };
      if (m[face]) return F[m[face]];
    }
    return F[face] !== undefined ? F[face] : 1;
  }

  function shade(hex, f) {
    let r = (hex >> 16) & 0xff, g = (hex >> 8) & 0xff, b = hex & 0xff;
    // warm bias when darkening (Mediterranean sun)
    if (f < 1) { r *= (1 + (1 - f) * 0.06); }
    r = Math.min(255, Math.round(r * f)); g = Math.min(255, Math.round(g * f)); b = Math.min(255, Math.round(b * f));
    return (r << 16) | (g << 8) | b;
  }
  function mix(a, b, t) {
    const ar = (a >> 16) & 0xff, ag = (a >> 8) & 0xff, ab = a & 0xff;
    const br = (b >> 16) & 0xff, bg = (b >> 8) & 0xff, bb = b & 0xff;
    return (Math.round(ar + (br - ar) * t) << 16) | (Math.round(ag + (bg - ag) * t) << 8) | Math.round(ab + (bb - ab) * t);
  }
  const lerp = (a, b, t) => ({ x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t });

  // ---- warm palette ------------------------------------------------------
  const MAT = {
    marble: 0xECE3CC, marbleWarm: 0xE2D4B4, marbleCool: 0xE7E2D2,
    stone: 0xCDBA8E, plinth: 0xC0AA78, plinthDk: 0xA88E5E,
    terracotta: 0xC15A33, terraTile: 0xCE6B40, terraDark: 0x95401F, terraGrout: 0x86381B,
    wood: 0x6F4A2A, woodLight: 0x8C6234, woodDk: 0x533619,
    thatch: 0xC2A258, thatchDk: 0x9A7C3C,
    mud: 0xC8A06C, mudDk: 0xA9824F, plaster: 0xE3CFA4, plasterDk: 0xC9B083,
    gold: 0xCEA53C, bronze: 0x9C7B3A, copper: 0x6FA890,
    red: 0xA8392E, redDeep: 0x84281F, blue: 0x35608A, blueDeep: 0x274B6E,
    ochre: 0xC98A2B, water: 0x3C7B92, waterDeep: 0x2E6072,
    leaf: 0x5E7E38, leafDk: 0x415C28, leafLt: 0x7C9A4C,
    cypress: 0x3E5A30, cypressDk: 0x2E4523, bush: 0x6C8C40, grass: 0x95A85A, grassDk: 0x768843,
    earth: 0xB1925F, earthDk: 0x927444, sand: 0xCDB888,
    ground: 0xB59B68, groundEdge: 0x8A6F45,
    flowerA: 0xD46A5A, flowerB: 0xE0C04A, flowerC: 0xCFCFE0,
    ink: 0x2B2A26, shadow: 0x4A3A24
  };

  function makeProj(W, D) {
    const ax = (W - D) * HALF_W, ay = (W + D) * HALF_H;
    return {
      W, D,
      p(gx, gy, gz) { return { x: (gx - gy) * HALF_W - ax, y: (gx + gy) * HALF_H - ay - (gz || 0) * Z_UNIT }; }
    };
  }

  function poly(g, pts, color, alpha) {
    const flat = []; for (const p of pts) flat.push(p.x, p.y);
    g.beginFill(color, alpha === undefined ? 1 : alpha); g.drawPolygon(flat); g.endFill();
  }
  function outlinePoly(g, pts, color, width, alpha) {
    g.lineStyle({ width: width || 1, color, alpha: alpha === undefined ? 1 : alpha, join: 'round' });
    g.moveTo(pts[0].x, pts[0].y);
    for (let i = 1; i < pts.length; i++) g.lineTo(pts[i].x, pts[i].y);
    g.lineTo(pts[0].x, pts[0].y); g.lineStyle(0);
  }
  function line(g, a, b, color, width, alpha) {
    g.lineStyle({ width: width || 1, color, alpha: alpha === undefined ? 1 : alpha, cap: 'round' });
    g.moveTo(a.x, a.y); g.lineTo(b.x, b.y); g.lineStyle(0);
  }
  function panelLeft(g, proj, gx, gy, z0, w, h, color, alpha) {
    poly(g, [proj.p(gx, gy, z0), proj.p(gx + w, gy, z0), proj.p(gx + w, gy, z0 + h), proj.p(gx, gy, z0 + h)], color, alpha);
  }
  function panelRight(g, proj, gx, gy, z0, d, h, color, alpha) {
    poly(g, [proj.p(gx, gy, z0), proj.p(gx, gy + d, z0), proj.p(gx, gy + d, z0 + h), proj.p(gx, gy, z0 + h)], color, alpha);
  }

  // ---- face texturing ----------------------------------------------------
  // quad order: [bl, br, tr, tl]  (bottom-left, bottom-right, top-right, top-left)
  function texFace(g, quad, kind, base, lit) {
    const [bl, br, tr, tl] = quad;
    const dk = shade(base, lit * 0.8), dk2 = shade(base, lit * 0.66), hl = shade(base, lit * 1.08);
    const hpx = Math.hypot(tl.x - bl.x, tl.y - bl.y); // wall height in px
    if (kind === 'ashlar' || kind === 'stone') {
      const rows = Math.max(2, Math.round(hpx / 11));
      for (let i = 1; i < rows; i++) {
        const t = i / rows;
        line(g, lerp(bl, tl, t), lerp(br, tr, t), dk, 1, 0.5);
      }
      for (let i = 0; i < rows; i++) {
        const t0 = i / rows, t1 = (i + 1) / rows, off = (i % 2) * 0.5;
        for (let u = off ? 0.5 : 1; u < 3; u++) {
          const uu = (u + off) / 3; if (uu >= 1) continue;
          line(g, lerp(lerp(bl, br, uu), lerp(tl, tr, uu), t0), lerp(lerp(bl, br, uu), lerp(tl, tr, uu), t1), dk, 1, 0.4);
        }
      }
    } else if (kind === 'plaster') {
      // darker base course + warm streaks + top highlight
      poly(g, [bl, br, lerp(br, tr, 0.14), lerp(bl, tl, 0.14)], dk, 0.55);
      line(g, lerp(bl, tl, 0.97), lerp(br, tr, 0.97), hl, 1.4, 0.5);
      for (let i = 0; i < 3; i++) {
        const u = 0.25 + i * 0.25;
        line(g, lerp(lerp(bl, br, u), lerp(tl, tr, u), 0.18), lerp(lerp(bl, br, u), lerp(tl, tr, u), 0.92), dk, 1, 0.12);
      }
    } else if (kind === 'marble') {
      line(g, lerp(bl, tl, 0.5), lerp(br, tr, 0.5), hl, 1, 0.3);
      poly(g, [bl, br, lerp(br, tr, 0.08), lerp(bl, tl, 0.08)], dk, 0.4);
      for (let i = 0; i < 2; i++) {
        const u = 0.35 + i * 0.3;
        line(g, lerp(lerp(bl, br, u), lerp(tl, tr, u), 0.1), lerp(lerp(bl, br, u), lerp(tl, tr, u), 0.95), dk2, 1, 0.1);
      }
    } else if (kind === 'wood') {
      const rows = Math.max(2, Math.round(hpx / 9));
      for (let i = 1; i < rows; i++) line(g, lerp(bl, tl, i / rows), lerp(br, tr, i / rows), dk, 1, 0.3);
    }
  }

  // ---- ground plot (textured) -------------------------------------------
  function ground(g, proj, W, D, opt) {
    opt = opt || {};
    const base = opt.color !== undefined ? opt.color : MAT.ground;
    const paved = opt.paved;
    for (let gx = 0; gx < W; gx++) {
      for (let gy = 0; gy < D; gy++) {
        const a = proj.p(gx, gy, 0), b = proj.p(gx + 1, gy, 0), c = proj.p(gx + 1, gy + 1, 0), d = proj.p(gx, gy + 1, 0);
        const v = ((gx * 7 + gy * 13) % 5) / 5;
        const col = paved ? shade(base, 0.97 + v * 0.06) : shade(mix(base, (gx + gy) % 2 ? MAT.grassDk : MAT.earth, 0.18 + v * 0.12), 1);
        poly(g, [a, b, c, d], col);
        if (opt.grid) outlinePoly(g, [a, b, c, d], paved ? shade(base, 0.8) : MAT.groundEdge, 1, paved ? 0.5 : 0.25);
        if (!paved && !opt.grid) {
          // grass tufts + pebbles
          const cx = (a.x + c.x) / 2, cy = (a.y + c.y) / 2;
          if (v > 0.55) {
            g.lineStyle({ width: 1, color: MAT.grass, alpha: 0.8 });
            for (let k = -1; k <= 1; k++) { g.moveTo(cx + k * 4 + v * 5, cy + 4); g.lineTo(cx + k * 4 + v * 5 - 1, cy - 1); }
            g.lineStyle(0);
          } else if (v < 0.2) { g.beginFill(MAT.earthDk, 0.5); g.drawCircle(cx - 6 + v * 30, cy, 1.6); g.endFill(); }
        }
      }
    }
    if (opt.edge) {
      const a = proj.p(0, 0, 0), b = proj.p(W, 0, 0), c = proj.p(W, D, 0), d = proj.p(0, D, 0);
      outlinePoly(g, [a, b, c, d], MAT.groundEdge, 1.5, 0.7);
    }
  }

  // ---- box (with optional textured faces) -------------------------------
  function box(g, proj, x0, y0, z0, w, d, h, baseColor, opt) {
    opt = opt || {};
    const x1 = x0 + w, y1 = y0 + d, z1 = z0 + h, P = (a, b, cz) => proj.p(a, b, cz);
    const T = [P(x0, y0, z1), P(x1, y0, z1), P(x1, y1, z1), P(x0, y1, z1)];
    const Lq = [P(x0, y1, z0), P(x1, y1, z0), P(x1, y1, z1), P(x0, y1, z1)]; // bl,br,tr,tl
    const Rq = [P(x1, y0, z0), P(x1, y1, z0), P(x1, y1, z1), P(x1, y0, z1)];
    const cTop = opt.topColor !== undefined ? opt.topColor : shade(baseColor, faceFactor('top'));
    const cL = shade(opt.leftColor !== undefined ? opt.leftColor : baseColor, faceFactor('left'));
    const cR = shade(opt.rightColor !== undefined ? opt.rightColor : baseColor, faceFactor('right'));
    poly(g, Lq, cL); poly(g, Rq, cR);
    if (opt.tex) {
      texFace(g, Lq, opt.tex, opt.leftColor !== undefined ? opt.leftColor : baseColor, faceFactor('left'));
      texFace(g, Rq, opt.tex, opt.rightColor !== undefined ? opt.rightColor : baseColor, faceFactor('right'));
    }
    if (opt.top !== false) poly(g, T, cTop);
    if (opt.outline) {
      const ow = opt.outlineW || 1, oc = opt.outlineColor || MAT.ink, oa = opt.outlineAlpha || 0.3;
      outlinePoly(g, Lq, oc, ow, oa); outlinePoly(g, Rq, oc, ow, oa);
      if (opt.top !== false) outlinePoly(g, T, oc, ow, oa);
    }
    return { T, L: Lq, R: Rq, x1, y1, z1 };
  }

  function steps(g, proj, x0, y0, z0, w, d, n, stepH, inset, mat) {
    mat = mat || MAT.stone; let zx = z0;
    for (let i = 0; i < n; i++) {
      const ins = inset * i;
      box(g, proj, x0 + ins, y0 + ins, zx, w - 2 * ins, d - 2 * ins, stepH, mat, { outline: true, outlineAlpha: 0.16 });
      zx += stepH;
    }
    return zx;
  }

  // ---- fluted column -----------------------------------------------------
  function column(g, proj, cx, cy, z0, h, rad, mat, opt) {
    opt = opt || {}; mat = mat || MAT.marble;
    const base = proj.p(cx, cy, z0), top = proj.p(cx, cy, z0 + h);
    const wpx = rad * TILE_W, half = wpx / 2;
    const capH = opt.capH !== undefined ? opt.capH : wpx * 0.5;
    const baseH = opt.baseH !== undefined ? opt.baseH : wpx * 0.34;
    const yTop = top.y + capH * 0.3, yBot = base.y - baseH * 0.3;
    const cLit = shade(mat, 1.13), cMid = shade(mat, 0.97), cDk = shade(mat, 0.76);
    // shaft strips
    g.beginFill(cLit); g.drawRect(base.x - half, yTop, half * 0.92, yBot - yTop); g.endFill();
    g.beginFill(cMid); g.drawRect(base.x - half * 0.08, yTop, half * 0.5, yBot - yTop); g.endFill();
    g.beginFill(cDk); g.drawRect(base.x + half * 0.42, yTop, half * 0.58, yBot - yTop); g.endFill();
    // flutes
    g.lineStyle({ width: 1, color: cDk, alpha: 0.45 });
    for (let i = -2; i <= 2; i++) { const fx = base.x + i * half * 0.34; g.moveTo(fx, yTop + 2); g.lineTo(fx, yBot - 2); }
    g.lineStyle(0);
    // capital: echinus + abacus
    const capW = half * 1.4;
    g.beginFill(shade(mat, 1.06)); g.drawRect(top.x - capW, yTop - capH * 0.5, capW * 2, capH * 0.5); g.endFill();
    g.beginFill(shade(mat, 1.1)); g.drawRect(top.x - capW * 1.12, yTop - capH, capW * 2.24, capH * 0.5); g.endFill();
    g.beginFill(shade(mat, 0.86)); g.drawRect(top.x - capW * 1.12, yTop - capH * 0.56, capW * 2.24, capH * 0.12); g.endFill();
    if (opt.ionic) {
      g.beginFill(shade(mat, 0.82));
      g.drawCircle(top.x - capW * 0.7, yTop - capH * 0.72, capH * 0.3);
      g.drawCircle(top.x + capW * 0.7, yTop - capH * 0.72, capH * 0.3); g.endFill();
      g.beginFill(shade(mat, 1.12));
      g.drawCircle(top.x - capW * 0.7, yTop - capH * 0.72, capH * 0.13);
      g.drawCircle(top.x + capW * 0.7, yTop - capH * 0.72, capH * 0.13); g.endFill();
    }
    // base
    g.beginFill(shade(mat, 1.05)); g.drawRect(base.x - half * 1.22, yBot, half * 2.44, baseH); g.endFill();
    g.beginFill(shade(mat, 0.8)); g.drawRect(base.x - half * 1.22, yBot + baseH * 0.62, half * 2.44, baseH * 0.38); g.endFill();
  }

  function colonnade(g, proj, gx0, gy0, gx1, gy1, z0, h, rad, count, mat, opt) {
    const pts = [];
    for (let i = 0; i < count; i++) { const t = count === 1 ? 0 : i / (count - 1); pts.push({ x: gx0 + (gx1 - gx0) * t, y: gy0 + (gy1 - gy0) * t }); }
    pts.sort((a, b) => (a.x + a.y) - (b.x + b.y));
    for (const p of pts) column(g, proj, p.x, p.y, z0, h, rad, mat, opt);
  }

  // ---- tiled terracotta roof helpers ------------------------------------
  function tileQuad(g, eaveL, eaveR, ridgeL, ridgeR, base, lit, opt) {
    opt = opt || {};
    poly(g, [eaveL, eaveR, ridgeR, ridgeL], shade(base, lit));
    const span = Math.hypot(ridgeL.x - eaveL.x, ridgeL.y - eaveL.y);
    const rows = Math.max(2, Math.round(span / 8));
    // courses (parallel to eave), slightly lighter toward ridge
    for (let i = 1; i <= rows; i++) {
      const t = i / rows;
      const a = lerp(eaveL, ridgeL, t), b = lerp(eaveR, ridgeR, t);
      line(g, a, b, shade(base, lit * 0.74), 1.3, 0.55);
      if (i < rows) { const t2 = (i + 0.5) / rows; line(g, lerp(eaveL, ridgeL, t2), lerp(eaveR, ridgeR, t2), shade(base, lit * 1.06), 1, 0.3); }
    }
    // pan seams (up the slope)
    const seams = Math.max(3, Math.round(Math.hypot(eaveR.x - eaveL.x, eaveR.y - eaveL.y) / 9));
    for (let i = 1; i < seams; i++) { const u = i / seams; line(g, lerp(eaveL, eaveR, u), lerp(ridgeL, ridgeR, u), shade(base, lit * 0.7), 1, 0.3); }
    // eave gutter + antefixes
    line(g, eaveL, eaveR, shade(base, lit * 0.6), 1.8, 0.7);
    if (opt.antefix !== false) {
      const na = Math.max(2, Math.round(Math.hypot(eaveR.x - eaveL.x, eaveR.y - eaveL.y) / 16));
      for (let i = 0; i <= na; i++) { const p = lerp(eaveL, eaveR, i / na); g.beginFill(shade(base, lit * 1.12)); g.drawCircle(p.x, p.y - 1, 1.5); g.endFill(); }
    }
  }

  function gableRoof(g, proj, x0, y0, zt, w, d, rh, mat, opt) {
    opt = opt || {}; mat = mat || MAT.terracotta;
    const o = opt.overhang !== undefined ? opt.overhang : 0.14, ridge = opt.ridge || 'y';
    const P = (a, b, cz) => proj.p(a, b, cz);
    const pedMat = opt.pediment !== undefined ? opt.pediment : MAT.marble;
    if (ridge === 'y') {
      const rx = x0 + w / 2;
      const eaveR1 = P(x0 + w + o, y0 - o, zt), eaveR2 = P(x0 + w + o, y0 + d + o, zt);
      const ridgeF = P(rx, y0 + d + o, zt + rh), ridgeB = P(rx, y0 - o, zt + rh);
      const eaveL1 = P(x0 - o, y0 + d + o, zt), eaveL2 = P(x0 - o, y0 - o, zt);
      // back-left slope (faint)
      poly(g, [eaveL1, eaveL2, ridgeB, ridgeF], shade(mat, faceFactor('slopeL') * 0.9));
      // right slope (visible, tiled)
      tileQuad(g, eaveR2, eaveR1, ridgeF, ridgeB, mat, faceFactor('slopeR'));
      // ridge cap
      line(g, ridgeB, ridgeF, shade(mat, faceFactor('slopeR') * 1.1), 2.4, 0.9);
      // front pediment
      const triL = P(x0 - o, y0 + d + o, zt), triR = P(x0 + w + o, y0 + d + o, zt), triTop = P(rx, y0 + d + o, zt + rh);
      poly(g, [triL, triR, triTop], shade(pedMat, faceFactor('gableLit')));
      if (opt.tympanum) poly(g, [P(x0 + w * 0.14, y0 + d + o, zt + rh * 0.1), P(x0 + w * 0.86, y0 + d + o, zt + rh * 0.1), P(rx, y0 + d + o, zt + rh * 0.82)], shade(opt.tympanum, faceFactor('gableLit')));
      // raking cornice + dentils
      line(g, triL, triTop, shade(pedMat, 0.62), 2, 0.6); line(g, triR, triTop, shade(pedMat, 0.62), 2, 0.6);
      line(g, triL, triR, shade(pedMat, 0.7), 2, 0.5);
      const dn = Math.max(3, Math.round(w * 3));
      for (let i = 1; i < dn; i++) { const p = lerp(triL, triR, i / dn); g.beginFill(shade(pedMat, 0.66), 0.5); g.drawRect(p.x - 1, p.y - 4, 2, 3); g.endFill(); }
      return [triL, triR, triTop];
    } else {
      const ry = y0 + d / 2;
      const eaveF1 = P(x0 - o, y0 + d + o, zt), eaveF2 = P(x0 + w + o, y0 + d + o, zt);
      const ridgeR = P(x0 + w + o, ry, zt + rh), ridgeL = P(x0 - o, ry, zt + rh);
      tileQuad(g, eaveF1, eaveF2, ridgeL, ridgeR, mat, faceFactor('slopeL'));
      line(g, ridgeL, ridgeR, shade(mat, faceFactor('slopeL') * 1.08), 2.4, 0.9);
      const triF = P(x0 + w + o, y0 + d + o, zt), triB = P(x0 + w + o, y0 - o, zt), triTop = P(x0 + w + o, ry, zt + rh);
      poly(g, [triF, triB, triTop], shade(pedMat, faceFactor('gableShade')));
      if (opt.tympanum) poly(g, [P(x0 + w + o, y0 + d * 0.14, zt + rh * 0.1), P(x0 + w + o, y0 + d * 0.86, zt + rh * 0.1), P(x0 + w + o, ry, zt + rh * 0.82)], shade(opt.tympanum, faceFactor('gableShade')));
      line(g, triF, triTop, shade(pedMat, 0.55), 2, 0.6); line(g, triB, triTop, shade(pedMat, 0.55), 2, 0.6);
      return [triF, triB, triTop];
    }
  }

  // hipped tiled roof (rectangular → ridge; square → pyramid)
  function hipRoof(g, proj, x0, y0, zt, w, d, rh, mat, opt) {
    opt = opt || {}; mat = mat || MAT.terracotta;
    const o = opt.overhang !== undefined ? opt.overhang : 0.12;
    const P = (a, b, cz) => proj.p(a, b, cz);
    const inset = Math.min(w, d) * 0.28;
    const rL = P(x0 + inset, y0 + d / 2, zt + rh), rR = P(x0 + w - inset, y0 + d / 2, zt + rh);
    const e = { ne: P(x0 - o, y0 - o, zt), nw: P(x0 + w + o, y0 - o, zt), sw: P(x0 + w + o, y0 + d + o, zt), se: P(x0 - o, y0 + d + o, zt) };
    // front (gy+) trapezoid — lit, tiled
    tileQuad(g, e.se, e.sw, rL, rR, mat, faceFactor('slopeL'));
    // right (gx+) trapezoid
    tileQuad(g, e.sw, e.nw, rR, rR, mat, faceFactor('slopeR'));
    // ridge + hips
    line(g, rL, rR, shade(mat, faceFactor('slopeL') * 1.1), 2.4, 0.9);
    line(g, e.se, rL, shade(mat, 0.6), 1.4, 0.6); line(g, e.sw, rL, shade(mat, 0.6), 1.4, 0.6); line(g, e.sw, rR, shade(mat, 0.6), 1.4, 0.6);
    return P(x0 + w / 2, y0 + d / 2, zt + rh);
  }

  function cylinder(g, proj, cx, cy, z0, rad, h, mat, opt) {
    opt = opt || {}; mat = mat || MAT.marble; const seg = opt.seg || 16;
    const top = [], bot = [];
    for (let i = 0; i <= seg; i++) { const a = (i / seg) * Math.PI * 2; const gx = cx + Math.cos(a) * rad, gy = cy + Math.sin(a) * rad; top.push(proj.p(gx, gy, z0 + h)); bot.push(proj.p(gx, gy, z0)); }
    for (let i = 0; i < seg; i++) {
      const ang = ((i + 0.5) / seg) * Math.PI * 2; if (Math.sin(ang) <= -0.25) continue;
      const lr = Math.cos(ang), f = 0.95 + lr * (SUN.dir === 'NE' ? 0.24 : -0.24);
      poly(g, [bot[i], bot[i + 1], top[i + 1], top[i]], shade(mat, Math.max(0.58, f)));
    }
    // courses
    for (let r = 1; r < Math.max(2, Math.round(h * Z_UNIT / 12)); r++) {
      const t = r / Math.max(2, Math.round(h * Z_UNIT / 12));
      g.lineStyle({ width: 1, color: shade(mat, 0.7), alpha: 0.3 });
      for (let i = 0; i < seg; i++) { const ang = ((i + 0.5) / seg) * Math.PI * 2; if (Math.sin(ang) <= -0.25) continue; const a = lerp(bot[i], top[i], t), b = lerp(bot[i + 1], top[i + 1], t); g.moveTo(a.x, a.y); g.lineTo(b.x, b.y); }
      g.lineStyle(0);
    }
    poly(g, top, shade(mat, faceFactor('top')));
    if (opt.outline) outlinePoly(g, top, MAT.ink, 1, 0.22);
    return z0 + h;
  }

  function project(proj, gx, gy, gz) { return proj.p(gx, gy, gz); }

  global.ISO = {
    TILE_W, TILE_H, HALF_W, HALF_H, Z_UNIT, SUN, MAT,
    shade, mix, lerp, faceFactor, line,
    makeProj, poly, outlinePoly, project, panelLeft, panelRight, texFace,
    ground, box, steps, column, colonnade, gableRoof, hipRoof, cylinder, tileQuad
  };
})(window);
