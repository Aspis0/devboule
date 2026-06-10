/* =========================================================================
   monuments_b.js — Monuments of Ancient Greece, part B (PixiJS v7)
   Olympieion (colossal Corinthian temple, partly ruined) ·
   Kolossos (Colossus of Rhodes) · Zeus Olympios (enthroned statue) ·
   Athēna Parthenos (standing statue of Pheidias)
   ========================================================================= */
(function (global) {
  'use strict';
  const M = ISO.MAT;
  global.MON = global.MON || {};
  const B = global.MON;

  function setup(W, D, opt) {
    const proj = ISO.makeProj(W, D);
    const c = new PIXI.Container();
    const g = new PIXI.Graphics();
    c.addChild(g);
    const out = !!(opt && opt.outline);
    const TEX = {};
    TEX[M.marble] = 'marble'; TEX[M.marbleCool] = 'marble'; TEX[M.marbleWarm] = 'plaster';
    TEX[M.stone] = 'ashlar'; TEX[M.plinth] = 'ashlar'; TEX[M.wood] = 'wood';
    const bx = (x, y, z, w, d, h, col, o) => {
      o = o || {};
      const t = o.tex !== undefined ? o.tex : TEX[col];
      return ISO.box(g, proj, x, y, z, w, d, h, col, Object.assign({ outline: out }, o, { tex: t }));
    };
    return { proj, c, g, out, bx, anims: [] };
  }

  // tiny acanthus suggestion under a column capital (Corinthian flavour)
  function corinthian(g, proj, cx, cy, ztop, rad) {
    const p = proj.p(cx, cy, ztop);
    g.beginFill(ISO.shade(M.marble, 0.82));
    g.drawPolygon([p.x - rad * 22, p.y + 4, p.x, p.y + 12, p.x + rad * 22, p.y + 4, p.x + rad * 14, p.y - 6, p.x - rad * 14, p.y - 6]);
    g.endFill();
    g.beginFill(ISO.shade(M.leafDk, 1.0), 0.5);
    for (let i = -1; i <= 1; i++) g.drawCircle(p.x + i * rad * 12, p.y + 3, 2.2);
    g.endFill();
  }

  // ====================== OLYMPIEION =====================================
  B.olympieion = function (opt) {
    const W = 5, D = 7;
    const s = setup(W, D, opt); const { proj, g, c, out, bx, anims } = s;
    const topZ = ISO.steps(g, proj, 0, 0, 0, W, D, 3, 0.2, 0.14, M.stone);
    const ins = 0.14 * 3, ix = ins, iy = ins, iw = W - 2 * ins, id = D - 2 * ins;
    const colH = 3.2, colR = 0.15; // colossal
    // a standing forest of very tall Corinthian columns (back + sides + front rows)
    const rows = [
      { gy: iy, n: 6, h: colH }, { gy: iy + id, n: 6, h: colH }
    ];
    // back row
    ISO.colonnade(g, proj, ix, iy, ix + iw, iy, topZ, colH, colR, 6, M.marble, { ionic: true, outline: out });
    for (let i = 0; i < 6; i++) corinthian(g, proj, ix + iw * i / 5, iy, topZ + colH, colR);
    // side rows
    ISO.colonnade(g, proj, ix, iy, ix, iy + id, topZ, colH, colR, 8, M.marble, { ionic: true });
    ISO.colonnade(g, proj, ix + iw, iy, ix + iw, iy + id, topZ, colH, colR, 8, M.marble, { ionic: true });
    // a surviving fragment of entablature spanning the back-left corner only
    bx(ix - 0.05, iy - 0.05, topZ + colH, iw * 0.45, 0.3, 0.26, M.marble);
    // front row (nearest) — but leave a GAP to suggest collapse
    [0, 1, 2, 4, 5].forEach(i => {
      ISO.column(g, proj, ix + iw * i / 5, iy + id, topZ, colH, colR, M.marble, { ionic: true, outline: out });
      corinthian(g, proj, ix + iw * i / 5, iy + id, topZ + colH, colR);
    });
    void rows;
    // a toppled column lying in front: a row of fallen drums + a capital
    const fy = D - 0.4;
    for (let k = 0; k < 6; k++) {
      const dx = 0.7 + k * 0.62;
      const dp = proj.p(dx, fy, 0.18);
      g.beginFill(ISO.shade(M.marble, k % 2 ? 0.92 : 1.04)); g.drawEllipse(dp.x, dp.y - 9, 13, 9); g.endFill();
      g.beginFill(ISO.shade(M.marble, 0.74)); g.drawEllipse(dp.x + 11, dp.y - 9, 3.4, 8.5); g.endFill();
      g.lineStyle({ width: 1, color: ISO.shade(M.marble, 0.6), alpha: 0.4 }); g.drawEllipse(dp.x, dp.y - 9, 13, 9); g.lineStyle(0);
    }
    PROP.cypress(g, proj, -0.3, D - 0.5, 0, 1.05); PROP.olive(g, proj, W + 0.3, D - 1.0, 0, 0.95);
    PROP.bush(g, proj, 0.2, D + 0.05, 0, 0.7, 5);
    void anims;
    return { container: c, body: g, anims, foot: [W, D] };
  };

  // ====================== KOLOSSOS (Colossus of Rhodes) ==================
  B.kolossos = function (opt) {
    const W = 3, D = 3;
    const s = setup(W, D, opt); const { proj, g, c, out, bx, anims } = s;
    // harbour water lapping the front strip
    const wD = 0.7;
    const wpts = [proj.p(0, D - wD, 0), proj.p(W, D - wD, 0), proj.p(W, D, 0), proj.p(0, D, 0)];
    const w = new ANIM.Water(wpts, 0.9); c.addChildAt(w.node, 0); anims.push(w);
    // stepped marble plinth
    const topZ = ISO.steps(g, proj, 0.2, 0.1, 0, W - 0.4, D - wD - 0.1, 3, 0.18, 0.16, M.marble);
    bx(0.5, 0.35, topZ, W - 1.0, D - wD - 0.55, 0.55, M.marbleCool); // pedestal block
    ISO.panelLeft(g, proj, 0.55, D - wD - 0.2, topZ + 0.12, W - 1.1, 0.3, ISO.shade(M.bronze, 0.85)); // dedication plaque
    const pz = topZ + 0.55;
    // the giant bronze Helios
    const base = proj.p(W / 2, (D - wD) / 2 + 0.15, pz);
    const rig = FIG.heroicMale(g, base.x, base.y, 2.3, { mat: M.bronze, cloth: M.copper, helios: true, torch: true });
    // beacon flame in the lifted torch
    const bc = new ANIM.Beacon(rig.torch.x, rig.torch.y - 2, 1.2); c.addChild(bc.node); anims.push(bc);
    void out;
    return { container: c, body: g, anims, foot: [W, D] };
  };

  // ====================== ZEUS OLYMPIOS (enthroned statue) ===============
  B.zeus = function (opt) {
    const W = 4, D = 4;
    const s = setup(W, D, opt); const { proj, g, c, out, bx, anims } = s;
    // naos floor + framing of the temple interior (Pheidias' workshop housing)
    ISO.box(g, proj, 0, 0, 0, W, D, 0.12, ISO.shade(M.stone, 1.04), { outline: out });
    // back wall + two flanking columns suggest the cella enclosing the statue
    bx(0.3, 0.2, 0.12, W - 0.6, 0.35, 3.0, ISO.shade(M.marbleCool, 0.96));
    ISO.colonnade(g, proj, 0.55, 0.7, 0.55, D - 1.2, 0.12, 2.7, 0.13, 3, M.marble, { outline: out });
    ISO.colonnade(g, proj, W - 0.55, 0.7, W - 0.55, D - 1.2, 0.12, 2.7, 0.13, 3, M.marble, { outline: out });
    // entablature beam across the front
    bx(0.4, 0.55, 0.12 + 2.7, W - 0.8, 0.3, 0.22, M.marble);
    ISO.gableRoof(g, proj, 0.35, 0.5, 0.12 + 2.92, W - 0.7, 0.45, 0.55, M.terracotta, { ridge: 'x', overhang: 0.16, tympanum: M.gold, outline: out });
    // reflecting oil pool in front of the throne (kept the ivory from cracking)
    const px0 = 0.7, px1 = W - 0.7, py0 = D - 1.0, py1 = D - 0.3;
    ISO.poly(g, [proj.p(px0 - 0.06, py0 - 0.06, 0.12), proj.p(px1 + 0.06, py0 - 0.06, 0.12), proj.p(px1 + 0.06, py1 + 0.06, 0.12), proj.p(px0 - 0.06, py1 + 0.06, 0.12)], ISO.shade(M.marble, 0.86));
    const wpts = [proj.p(px0, py0, 0.1), proj.p(px1, py0, 0.1), proj.p(px1, py1, 0.1), proj.p(px0, py1, 0.1)];
    const w = new ANIM.Water(wpts, 0.7); c.addChild(w.node); anims.push(w);
    // the enthroned chryselephantine Zeus on a low dais — colossal, head near the roof
    bx(W / 2 - 1.0, 0.8, 0.12, 2.0, 1.15, 0.42, ISO.shade(M.stone, 0.95));
    const base = proj.p(W / 2, 1.5, 0.54);
    FIG.enthroned(g, base.x, base.y, 2.05, { gold: M.gold, ivory: M.marble });
    return { container: c, body: g, anims, foot: [W, D] };
  };

  // ====================== ATHENA PARTHENOS (standing statue) =============
  B.athena = function (opt) {
    const W = 3, D = 3;
    const s = setup(W, D, opt); const { proj, g, c, out, bx, anims } = s;
    // shallow naos floor
    ISO.box(g, proj, 0, 0, 0, W, D, 0.1, ISO.shade(M.stone, 1.05), { outline: out });
    // two framing columns at the back corners (interior of the Parthenon cella)
    ISO.colonnade(g, proj, 0.45, 0.4, 0.45, D - 1.0, 0.1, 2.8, 0.12, 2, M.marble, { outline: out });
    ISO.colonnade(g, proj, W - 0.45, 0.4, W - 0.45, D - 1.0, 0.1, 2.8, 0.12, 2, M.marble, { outline: out });
    bx(0.35, 0.25, 0.1 + 2.8, W - 0.7, 0.3, 0.2, M.marble); // upper beam
    // tall sculpted pedestal with a relief band (Birth of Pandora frieze)
    bx(W / 2 - 0.95, D / 2 - 0.6, 0.1, 1.9, 1.2, 0.85, M.marbleCool);
    ISO.panelLeft(g, proj, W / 2 - 0.95, D / 2 + 0.6, 0.3, 1.9, 0.45, ISO.shade(M.marbleWarm, 0.84));
    for (let i = 1; i < 7; i++) { const u = i / 7; ISO.line(g, proj.p(W / 2 - 0.95 + u * 1.9, D / 2 + 0.6, 0.34), proj.p(W / 2 - 0.95 + u * 1.9, D / 2 + 0.6, 0.72), ISO.shade(M.ink, 1.3), 1, 0.3); }
    // the standing gold-and-ivory Athena
    const base = proj.p(W / 2, D / 2, 0.95);
    FIG.goddess(g, base.x, base.y, 2.0, { gold: M.gold, ivory: M.marble });
    return { container: c, body: g, anims, foot: [W, D] };
  };

})(window);
