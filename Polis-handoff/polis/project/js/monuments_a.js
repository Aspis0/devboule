/* =========================================================================
   monuments_a.js — Monuments of Ancient Greece, part A (PixiJS v7)
   Parthenon · Erechtheion · Artemision · Tholos · Horologion (Tower of
   the Winds) · Mausoleion · Propylaia · Bomos (Great Altar of Pergamon)
   Each builder: fn(opt) -> { container, body, anims, foot:[W,D] }
   Built from the same ISO/PROP/ANIM/FIG primitive language as the polis
   building atlas — just at heroic scale.
   ========================================================================= */
(function (global) {
  'use strict';
  const M = ISO.MAT;
  global.MON = global.MON || {};
  const B = global.MON;

  // shared registry (display + accent + caption) --------------------------
  global.MON_META = {
    order: ['parthenon', 'erechtheion', 'artemision', 'tholos', 'horologion',
      'mausoleion', 'propylaia', 'bomos', 'olympieion', 'kolossos',
      'zeus', 'athena'],
    info: {
      parthenon: { name: 'Parthenōn', cat: 'tempio', sub: 'Acropoli · Atene', accent: M.blue },
      erechtheion: { name: 'Erechtheion', cat: 'tempio', sub: 'Loggia delle Cariatidi', accent: M.blue },
      artemision: { name: 'Artemision', cat: 'tempio', sub: 'Artemide · Efeso ⭐', accent: M.gold },
      tholos: { name: 'Tholos', cat: 'tempio', sub: 'Santuario · Delfi', accent: M.terracotta },
      horologion: { name: 'Horologion', cat: 'civile', sub: 'Torre dei Venti', accent: M.copper },
      mausoleion: { name: 'Mausōleion', cat: 'tomba', sub: 'Alicarnasso ⭐', accent: M.bronze },
      propylaia: { name: 'Propylaia', cat: 'portale', sub: 'Ingresso all’Acropoli', accent: M.blue },
      bomos: { name: 'Bōmos', cat: 'altare', sub: 'Grande Altare · Pergamo', accent: M.red },
      olympieion: { name: 'Olympieion', cat: 'tempio', sub: 'Zeus Olimpio · Atene', accent: M.gold },
      kolossos: { name: 'Kolossos', cat: 'statua', sub: 'Colosso di Rodi ⭐', accent: M.bronze },
      zeus: { name: 'Zeus Olympios', cat: 'statua', sub: 'Olimpia ⭐', accent: M.gold },
      athena: { name: 'Athēna Parthenos', cat: 'statua', sub: 'di Fidia · Atene', accent: M.gold }
    }
  };

  // ---- shared setup ------------------------------------------------------
  function setup(W, D, opt) {
    const proj = ISO.makeProj(W, D);
    const c = new PIXI.Container();
    const g = new PIXI.Graphics();
    c.addChild(g);
    const out = !!(opt && opt.outline);
    const TEX = {};
    TEX[M.marble] = 'marble'; TEX[M.marbleCool] = 'marble'; TEX[M.marbleWarm] = 'plaster';
    TEX[M.plaster] = 'plaster'; TEX[M.stone] = 'ashlar';
    TEX[M.plinth] = 'ashlar'; TEX[M.plinthDk] = 'ashlar'; TEX[M.wood] = 'wood';
    const bx = (x, y, z, w, d, h, col, o) => {
      o = o || {};
      const t = o.tex !== undefined ? o.tex : TEX[col];
      return ISO.box(g, proj, x, y, z, w, d, h, col, Object.assign({ outline: out }, o, { tex: t }));
    };
    return { proj, c, g, out, bx, anims: [] };
  }

  // ---- conical / pyramidal tiled roof (for round & polygonal buildings) -
  function cone(g, proj, cx, cy, z0, r, h, mat, opt) {
    opt = opt || {}; mat = mat || M.terracotta; const seg = opt.seg || 16;
    const apex = proj.p(cx, cy, z0 + h), base = [];
    for (let i = 0; i <= seg; i++) { const a = (i / seg) * Math.PI * 2 + (opt.rot || 0); base.push(proj.p(cx + Math.cos(a) * r, cy + Math.sin(a) * r, z0)); }
    for (let i = 0; i < seg; i++) {
      const ang = ((i + 0.5) / seg) * Math.PI * 2 + (opt.rot || 0);
      if (Math.sin(ang) <= -0.34) continue;
      const lr = Math.cos(ang), f = 0.96 + lr * (ISO.SUN.dir === 'NE' ? 0.2 : -0.2);
      ISO.poly(g, [base[i], base[i + 1], apex], ISO.shade(mat, Math.max(0.6, f)));
      for (let r2 = 1; r2 < 4; r2++) { const t = r2 / 4; ISO.line(g, ISO.lerp(base[i], apex, t), ISO.lerp(base[i + 1], apex, t), ISO.shade(mat, Math.max(0.5, f * 0.78)), 1.1, 0.45); }
      ISO.line(g, base[i], apex, ISO.shade(mat, 0.58), 1, 0.5);
    }
    // finial
    g.beginFill(ISO.shade(mat, 1.1)); g.drawCircle(apex.x, apex.y, 2.4); g.endFill();
    return apex;
  }

  // ---- ring of columns around a circle; draws back→front around `body` --
  function ringColumns(g, proj, cx, cy, r, z0, h, rad, n, mat, opt, which) {
    const pts = [];
    for (let i = 0; i < n; i++) { const a = (i / n) * Math.PI * 2 + (opt && opt.rot || 0); pts.push({ x: cx + Math.cos(a) * r, y: cy + Math.sin(a) * r, s: Math.sin(a) }); }
    const sel = pts.filter(p => which === 'front' ? p.s >= -0.05 : p.s < -0.05).sort((a, b) => (a.x + a.y) - (b.x + b.y));
    for (const p of sel) ISO.column(g, proj, p.x, p.y, z0, h, rad, mat, opt);
  }

  // ====================== PARTHENON ======================================
  B.parthenon = function (opt) {
    const W = 5, D = 8;
    const s = setup(W, D, opt); const { proj, g, c, out, bx, anims } = s;
    // crepidoma (3-step stylobate)
    const topZ = ISO.steps(g, proj, 0, 0, 0, W, D, 3, 0.2, 0.18, M.stone);
    const ins = 0.18 * 3, ix = ins, iy = ins, iw = W - 2 * ins, id = D - 2 * ins;
    const colH = 2.05, colR = 0.135, frontN = 8, sideN = 17 > 12 ? 13 : 11;
    // back colonnade
    ISO.colonnade(g, proj, ix, iy, ix + iw, iy, topZ, colH, colR, frontN, M.marble, { outline: out });
    // cella (naos) with porch walls
    const cw = iw * 0.62, cd = id * 0.8, cx0 = ix + (iw - cw) / 2, cy0 = iy + (id - cd) / 2;
    bx(cx0, cy0, topZ, cw, cd, colH * 0.96, M.marbleCool);
    ISO.panelLeft(g, proj, cx0 + cw * 0.34, cy0 + cd, topZ, cw * 0.32, colH * 0.62, ISO.shade(M.wood, 0.7)); // bronze door
    // side colonnades
    ISO.colonnade(g, proj, ix, iy, ix, iy + id, topZ, colH, colR, sideN, M.marble, { outline: out });
    ISO.colonnade(g, proj, ix + iw, iy, ix + iw, iy + id, topZ, colH, colR, sideN, M.marble, { outline: out });
    // entablature + coloured frieze (triglyphs/metopes hinted by band)
    const ez = topZ + colH;
    bx(ix - 0.08, iy - 0.08, ez, iw + 0.16, id + 0.16, 0.2, M.marble);
    ISO.panelLeft(g, proj, ix - 0.08, iy + id + 0.08, ez + 0.04, iw + 0.16, 0.11, ISO.shade(M.blue, ISO.faceFactor('left')));
    ISO.panelRight(g, proj, ix + iw + 0.08, iy - 0.08, ez + 0.04, id + 0.16, 0.11, ISO.shade(M.red, ISO.faceFactor('right')));
    // gabled tiled roof with sculpted pediment
    ISO.gableRoof(g, proj, ix - 0.08, iy - 0.08, ez + 0.2, iw + 0.16, id + 0.16, 0.78, M.terracotta,
      { ridge: 'y', overhang: 0.24, tympanum: M.blue, outline: out });
    // front colonnade (nearest)
    ISO.colonnade(g, proj, ix, iy + id, ix + iw, iy + id, topZ, colH, colR, frontN, M.marble, { outline: out });
    // gold acroteria at the three apexes
    const apx = proj.p(ix + iw / 2, iy + id + 0.16, ez + 0.2 + 0.78);
    g.beginFill(M.gold); g.drawCircle(apx.x, apx.y - 3, 4.4); g.endFill();
    [iy - 0.08, iy + id + 0.08].forEach(yy => { const p = proj.p(ix - 0.08, yy, ez + 0.2); g.beginFill(M.gold); g.drawCircle(p.x, p.y - 2, 2.6); g.endFill(); });
    return { container: c, body: g, anims, foot: [W, D] };
  };

  // ====================== ERECHTHEION ====================================
  B.erechtheion = function (opt) {
    const W = 5, D = 5;
    const s = setup(W, D, opt); const { proj, g, c, out, bx, anims } = s;
    const topZ = ISO.steps(g, proj, 0, 0, 0, W, D, 2, 0.16, 0.12, M.stone);
    // main cella block (the asymmetric temple body) at back-left
    const mw = 3.1, md = 3.0, mx = 0.3, my = 0.3;
    bx(mx, my, topZ, mw, md, 1.7, M.marbleCool);
    // Ionic prostyle porch on the cella front
    ISO.colonnade(g, proj, mx + 0.2, my + md, mx + mw - 0.2, my + md, topZ, 1.7, 0.12, 4, M.marble, { ionic: true, outline: out });
    bx(mx - 0.05, my - 0.05, topZ + 1.7, mw + 0.1, md + 0.1, 0.18, M.marble);
    ISO.gableRoof(g, proj, mx - 0.05, my - 0.05, topZ + 1.88, mw + 0.1, md + 0.1, 0.6, M.terracotta, { ridge: 'y', overhang: 0.2, tympanum: M.blue, outline: out });
    // ---- Porch of the Maidens (Caryatids) on the front-right corner ----
    const px = 2.6, py = 2.5, pw = 2.3, pd = 1.8;
    bx(px, py, topZ, pw, pd, 0.5, M.stone); // raised porch platform
    const pz = topZ + 0.5;
    const scC = 1.75;
    const frontY = py + pd - 0.12, backY = py + 0.5;
    const fxs = [px + 0.35, px + pw * 0.4, px + pw * 0.6, px + pw - 0.35];
    const bxs = [px + pw * 0.34, px + pw * 0.66];
    // 2 maidens set back (peek between the front four)
    bxs.forEach(cxx => { const pt = proj.p(cxx, backY, pz); FIG.caryatid(g, pt.x, pt.y, scC, M.marbleCool); });
    // slim architrave the maidens carry — sits right on their heads, shallow
    const az = pz + 1.74;
    bx(px - 0.05, py + 0.35, az, pw + 0.1, pd - 0.45, 0.22, M.marble);
    ISO.panelLeft(g, proj, px - 0.05, frontY + 0.04, az + 0.02, pw + 0.1, 0.12, ISO.shade(M.blue, ISO.faceFactor('left')));
    // four maidens across the front (drawn last → clearly visible)
    fxs.forEach(cxx => { const pt = proj.p(cxx, frontY, pz); FIG.caryatid(g, pt.x, pt.y, scC, M.marbleCool); });
    PROP.cypress(g, proj, -0.3, D - 0.5, 0, 1.0); PROP.olive(g, proj, W + 0.3, D - 0.7, 0, 0.9);
    return { container: c, body: g, anims, foot: [W, D] };
  };

  // ====================== ARTEMISION (Temple of Artemis, Ephesus) ========
  B.artemision = function (opt) {
    const W = 6, D = 9;
    const s = setup(W, D, opt); const { proj, g, c, out, bx, anims } = s;
    // tall stepped krepis
    const topZ = ISO.steps(g, proj, 0, 0, 0, W, D, 4, 0.18, 0.16, M.stone);
    const ins = 0.16 * 4, ix = ins, iy = ins, iw = W - 2 * ins, id = D - 2 * ins;
    const colH = 2.5, colR = 0.13, frontN = 8, sideN = 16;
    // DIPTERAL: double rows of Ionic columns. Inner + outer, back first.
    const inset2 = 0.7;
    ISO.colonnade(g, proj, ix, iy, ix + iw, iy, topZ, colH, colR, frontN, M.marble, { ionic: true, outline: out }); // outer back
    ISO.colonnade(g, proj, ix + inset2, iy + inset2, ix + iw - inset2, iy + inset2, topZ, colH * 0.98, colR, frontN, M.marble, { ionic: true }); // inner back
    // cella
    const cw = iw * 0.5, cd = id * 0.74, cx0 = ix + (iw - cw) / 2, cy0 = iy + (id - cd) / 2;
    bx(cx0, cy0, topZ, cw, cd, colH * 1.02, M.marbleCool);
    ISO.panelLeft(g, proj, cx0 + cw * 0.32, cy0 + cd, topZ, cw * 0.36, colH * 0.5, ISO.shade(M.gold, 0.7));
    // side double colonnades
    [ix, ix + inset2].forEach((xx, k) => ISO.colonnade(g, proj, xx, iy + (k ? inset2 : 0), xx, iy + id - (k ? inset2 : 0), topZ, colH * (k ? 0.98 : 1), colR, sideN - k * 2, M.marble, { ionic: true, outline: out && !k }));
    [ix + iw, ix + iw - inset2].forEach((xx, k) => ISO.colonnade(g, proj, xx, iy + (k ? inset2 : 0), xx, iy + id - (k ? inset2 : 0), topZ, colH * (k ? 0.98 : 1), colR, sideN - k * 2, M.marble, { ionic: true, outline: out && !k }));
    // entablature + roof
    const ez = topZ + colH;
    bx(ix - 0.1, iy - 0.1, ez, iw + 0.2, id + 0.2, 0.22, M.marble);
    ISO.panelLeft(g, proj, ix - 0.1, iy + id + 0.1, ez + 0.04, iw + 0.2, 0.12, ISO.shade(M.red, ISO.faceFactor('left')));
    ISO.gableRoof(g, proj, ix - 0.1, iy - 0.1, ez + 0.22, iw + 0.2, id + 0.2, 0.85, M.terracotta, { ridge: 'y', overhang: 0.26, tympanum: M.gold, outline: out });
    // front double colonnade (nearest)
    ISO.colonnade(g, proj, ix + inset2, iy + id - inset2, ix + iw - inset2, iy + id - inset2, topZ, colH * 0.98, colR, frontN, M.marble, { ionic: true });
    ISO.colonnade(g, proj, ix, iy + id, ix + iw, iy + id, topZ, colH, colR, frontN, M.marble, { ionic: true, outline: out });
    const apx = proj.p(ix + iw / 2, iy + id + 0.2, ez + 0.22 + 0.85);
    g.beginFill(M.gold); g.drawCircle(apx.x, apx.y - 3, 4.6); g.endFill();
    return { container: c, body: g, anims, foot: [W, D] };
  };

  // ====================== THOLOS (round temple, Delphi) ==================
  B.tholos = function (opt) {
    const W = 4, D = 4;
    const s = setup(W, D, opt); const { proj, g, c, out } = s;
    const cx = W / 2, cy = D / 2;
    // round stepped base (3 shrinking discs)
    let z = 0;
    [1.85, 1.62, 1.42].forEach((rr, i) => { z = ISO.cylinder(g, proj, cx, cy, z, rr, 0.16, M.stone, { seg: 28, outline: out }); });
    const z0 = z, colH = 1.5, colR = 0.12, n = 14, rCol = 1.5;
    // back columns
    ringColumns(g, proj, cx, cy, rCol, z0, colH, colR, n, M.marble, { outline: out }, 'back');
    // inner cella drum
    ISO.cylinder(g, proj, cx, cy, z0, 0.95, colH * 0.94, M.marbleCool, { seg: 24, outline: out });
    ISO.panelLeft(g, proj, cx - 0.3, cy + 0.92, z0, 0.6, colH * 0.55, ISO.shade(M.wood, 0.7));
    // circular entablature ring
    const ez = z0 + colH;
    ISO.cylinder(g, proj, cx, cy, ez, rCol + 0.18, 0.18, M.marble, { seg: 28 });
    // conical tiled roof
    cone(g, proj, cx, cy, ez + 0.18, rCol + 0.22, 1.05, M.terracotta, { seg: 24 });
    // front columns (nearest, overlap entablature base)
    ringColumns(g, proj, cx, cy, rCol, z0, colH, colR, n, M.marble, { outline: out }, 'front');
    PROP.cypress(g, proj, -0.2, D - 0.4, 0, 0.95); PROP.cypress(g, proj, W + 0.2, D - 0.4, 0, 0.95);
    return { container: c, body: g, anims: s.anims, foot: [W, D] };
  };

  // ====================== HOROLOGION (Tower of the Winds) ================
  B.horologion = function (opt) {
    const W = 3, D = 3;
    const s = setup(W, D, opt); const { proj, g, c, out, anims } = s;
    const cx = W / 2, cy = D / 2;
    const z0 = ISO.cylinder(g, proj, cx, cy, 0, 1.15, 0.18, M.stone, { seg: 8, outline: out });
    // octagonal marble shaft
    const shaftH = 2.6;
    ISO.cylinder(g, proj, cx, cy, z0, 1.0, shaftH, M.marble, { seg: 8, outline: out });
    // sculpted frieze band of the winds near the top (darker relief ring)
    ISO.cylinder(g, proj, cx, cy, z0 + shaftH - 0.5, 1.04, 0.42, ISO.shade(M.marbleWarm, 0.9), { seg: 8 });
    // little sundial gnomon lines on the lit faces
    for (let i = 0; i < 4; i++) {
      const a = (i / 8) * Math.PI * 2 - Math.PI * 0.1;
      const fx = cx + Math.cos(a) * 1.0, fy = cy + Math.sin(a) * 1.0;
      if (Math.sin(a) <= -0.2) continue;
      const p0 = proj.p(fx, fy, z0 + 0.8), p1 = proj.p(fx, fy, z0 + 1.7);
      ISO.line(g, p0, p1, ISO.shade(M.stone, 0.7), 1, 0.4);
    }
    // 8-sided pyramidal roof
    const ez = z0 + shaftH;
    cone(g, proj, cx, cy, ez, 1.12, 1.2, M.terracotta, { seg: 8 });
    // bronze Triton weathervane on top
    const tp = proj.p(cx, cy, ez + 1.2);
    const fg = new ANIM.Flag(tp.x, tp.y - 2, 0.95, M.copper); c.addChild(fg.node); anims.push(fg);
    return { container: c, body: g, anims, foot: [W, D] };
  };

  // ====================== MAUSOLEION (Mausoleum at Halicarnassus) ========
  B.mausoleion = function (opt) {
    const W = 4, D = 4;
    const s = setup(W, D, opt); const { proj, g, c, out, bx, anims } = s;
    // tall stepped podium
    let z = 0;
    z = ISO.steps(g, proj, 0, 0, 0, W, D, 2, 0.22, 0.1, M.stone);
    bx(0.45, 0.45, z, W - 0.9, D - 0.9, 1.6, M.marbleCool); // main podium block
    // relief frieze band on the podium
    ISO.panelLeft(g, proj, 0.45, D - 0.45, z + 0.6, W - 0.9, 0.4, ISO.shade(M.marbleWarm, 0.86));
    ISO.panelRight(g, proj, W - 0.45, 0.45, z + 0.6, D - 0.9, 0.4, ISO.shade(M.stone, 0.8));
    z += 1.6;
    // colonnade tier (Ionic peristyle) around a cella drum
    const ix = 0.6, iy = 0.6, iw = W - 1.2, id = D - 1.2, colH = 1.4, colR = 0.1;
    bx(ix + 0.25, iy + 0.25, z, iw - 0.5, id - 0.5, colH * 0.95, M.marble); // inner cella
    ISO.colonnade(g, proj, ix, iy, ix + iw, iy, z, colH, colR, 5, M.marble, { ionic: true, outline: out });
    ISO.colonnade(g, proj, ix, iy, ix, iy + id, z, colH, colR, 5, M.marble, { ionic: true });
    ISO.colonnade(g, proj, ix + iw, iy, ix + iw, iy + id, z, colH, colR, 5, M.marble, { ionic: true });
    const ez = z + colH;
    bx(ix - 0.1, iy - 0.1, ez, iw + 0.2, id + 0.2, 0.2, M.marble); // entablature
    ISO.colonnade(g, proj, ix, iy + id, ix + iw, iy + id, z, colH, colR, 5, M.marble, { ionic: true, outline: out });
    // stepped pyramid roof (24 → 7 steps abstracted)
    const pTop = ISO.steps(g, proj, ix - 0.05, iy - 0.05, ez + 0.2, iw + 0.1, id + 0.1, 7, 0.14, 0.1, M.marbleCool);
    // crowning quadriga
    const qp = proj.p(W / 2, D / 2 + 0.4, pTop);
    FIG.quadriga(g, qp.x, qp.y, 1.0, M.bronze);
    return { container: c, body: g, anims, foot: [W, D] };
  };

  // ====================== PROPYLAIA (Acropolis gateway) ==================
  B.propylaia = function (opt) {
    const W = 5, D = 3;
    const s = setup(W, D, opt); const { proj, g, c, out, bx, anims } = s;
    const topZ = ISO.steps(g, proj, 0, 0, 0, W, D, 3, 0.18, 0.12, M.stone);
    // side wings (lower blocks flanking the central passage)
    bx(0.3, 0.3, topZ, 1.0, D - 0.6, 1.6, M.marbleCool);
    bx(W - 1.3, 0.3, topZ, 1.0, D - 0.6, 1.6, M.marbleCool);
    // back wall with the open central gate
    bx(1.3, 0.3, topZ, W - 2.6, 0.3, 2.1, M.marble);
    ISO.panelLeft(g, proj, W / 2 - 0.55, 0.6, topZ, 1.1, 1.55, ISO.shade(M.ink, 1.5)); // gate shadow opening
    // tall Doric front colonnade with a wide central intercolumniation (the gate)
    const colH = 2.2, colR = 0.13;
    [0.55, 1.05].forEach(x => ISO.column(g, proj, x, D - 0.3, topZ, colH, colR, M.marble, { outline: out }));
    [W - 0.55, W - 1.05].forEach(x => ISO.column(g, proj, x, D - 0.3, topZ, colH, colR, M.marble, { outline: out }));
    ISO.column(g, proj, W / 2 - 0.8, D - 0.3, topZ, colH, colR, M.marble, { outline: out });
    ISO.column(g, proj, W / 2 + 0.8, D - 0.3, topZ, colH, colR, M.marble, { outline: out });
    // entablature spanning the full front + pediment
    const ez = topZ + colH;
    bx(0.2, D - 0.55, ez, W - 0.4, 0.4, 0.22, M.marble);
    ISO.gableRoof(g, proj, 0.2, D - 0.6, ez + 0.22, W - 0.4, 0.5, 0.7, M.terracotta, { ridge: 'x', overhang: 0.2, tympanum: M.blue, outline: out });
    // wing roofs
    ISO.gableRoof(g, proj, 0.3, 0.3, topZ + 1.6, 1.0, D - 0.6, 0.4, M.terracotta, { ridge: 'y', overhang: 0.12, outline: out });
    ISO.gableRoof(g, proj, W - 1.3, 0.3, topZ + 1.6, 1.0, D - 0.6, 0.4, M.terracotta, { ridge: 'y', overhang: 0.12, outline: out });
    void anims;
    PROP.statue(g, proj, 0.2, D + 0.05, 0, 0.85); PROP.statue(g, proj, W - 0.2, D + 0.05, 0, 0.85);
    return { container: c, body: g, anims, foot: [W, D] };
  };

  // ====================== BOMOS (Great Altar of Pergamon) ================
  B.bomos = function (opt) {
    const W = 5, D = 5;
    const s = setup(W, D, opt); const { proj, g, c, out, bx, anims } = s;
    // broad stepped podium
    let z = ISO.steps(g, proj, 0, 0, 0, W, D, 2, 0.16, 0.08, M.stone);
    bx(0.3, 0.3, z, W - 0.6, D - 0.6, 1.5, M.marbleCool); // main mass
    // gigantomachy frieze band wrapping the base (deep relief → dark band)
    ISO.panelLeft(g, proj, 0.3, D - 0.3, z + 0.15, W - 0.6, 0.7, ISO.shade(M.stone, 0.66));
    ISO.panelRight(g, proj, W - 0.3, 0.3, z + 0.15, D - 0.6, 0.7, ISO.shade(M.stone, 0.56));
    // relief tick marks to suggest carved figures
    for (let i = 1; i < 9; i++) { const u = i / 9; ISO.line(g, proj.p(0.3 + u * (W - 0.6), D - 0.3, z + 0.18), proj.p(0.3 + u * (W - 0.6), D - 0.3, z + 0.82), ISO.shade(M.ink, 1.3), 1, 0.3); }
    const pz = z + 1.5;
    // grand frontal staircase cut into the centre
    const sw = 2.0, sx = W / 2 - sw / 2;
    ISO.steps(g, proj, sx, D - 0.3, z, sw, 1.0, 6, 0.25, 0.0, ISO.shade(M.marble, 0.98));
    // top platform
    bx(0.3, 0.3, pz, W - 0.6, D - 0.6, 0.12, M.marble);
    // U-shaped Ionic colonnade (back + two sides, open over the stairs)
    const cz = pz + 0.12, colH = 1.5, colR = 0.1;
    ISO.colonnade(g, proj, 0.6, 0.6, W - 0.6, 0.6, cz, colH, colR, 6, M.marble, { ionic: true, outline: out });
    ISO.colonnade(g, proj, 0.6, 0.6, 0.6, D - 0.9, cz, colH, colR, 5, M.marble, { ionic: true });
    ISO.colonnade(g, proj, W - 0.6, 0.6, W - 0.6, D - 0.9, cz, colH, colR, 5, M.marble, { ionic: true });
    // entablature over the back/sides
    bx(0.5, 0.5, cz + colH, W - 1.0, 0.35, 0.18, M.marble);
    // sacred fire on the altar table at centre
    bx(W / 2 - 0.45, 0.9, cz, 0.9, 0.7, 0.4, M.stone);
    const fp = proj.p(W / 2, 1.25, cz + 0.4);
    const fl = new ANIM.Flame(fp.x, fp.y, 1.5); c.addChild(fl.node); anims.push(fl);
    PROP.cypress(g, proj, -0.25, D - 0.4, 0, 1.0); PROP.cypress(g, proj, W + 0.25, D - 0.4, 0, 1.0);
    return { container: c, body: g, anims, foot: [W, D] };
  };

})(window);
