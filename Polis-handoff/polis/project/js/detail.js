/* =========================================================================
   detail.js — Mediterranean props & greenery (PixiJS v7)
   Cypress, bushes, olive trees, garden beds, amphorae, statues, urns,
   fountains, hedges. Drawn at a projected ground point (screen-space
   billboards) so they read crisply at any building scale. The "lived-in"
   Caesar-III layer.
   ========================================================================= */
(function (global) {
  'use strict';
  const M = ISO.MAT, S = ISO.shade;
  const rnd = (seed) => { const x = Math.sin(seed * 99.13) * 43758.5; return x - Math.floor(x); };

  function cypress(g, proj, gx, gy, z, sc) {
    sc = sc || 1; const p = proj.p(gx, gy, z || 0);
    g.beginFill(M.shadow, 0.16); g.drawEllipse(p.x + 4 * sc, p.y, 8 * sc, 3 * sc); g.endFill();
    g.beginFill(M.woodDk); g.drawRect(p.x - 1.2 * sc, p.y - 8 * sc, 2.4 * sc, 8 * sc); g.endFill();
    g.beginFill(M.cypressDk); g.drawEllipse(p.x, p.y - 22 * sc, 6 * sc, 20 * sc); g.endFill();
    g.beginFill(M.cypress); g.drawEllipse(p.x - 1.6 * sc, p.y - 24 * sc, 4 * sc, 18 * sc); g.endFill();
    g.beginFill(S(M.cypress, 1.18)); g.drawEllipse(p.x - 2.4 * sc, p.y - 28 * sc, 1.8 * sc, 9 * sc); g.endFill();
  }

  function bush(g, proj, gx, gy, z, sc, seed) {
    sc = sc || 1; seed = seed || gx * 3 + gy; const p = proj.p(gx, gy, z || 0);
    g.beginFill(M.shadow, 0.16); g.drawEllipse(p.x + 3 * sc, p.y, 9 * sc, 3.2 * sc); g.endFill();
    const blobs = [[0, 0, 7], [-5, 2, 5], [5, 1, 5.5], [1, -4, 6]];
    blobs.forEach((b, i) => { g.beginFill(i % 2 ? M.bush : M.leafDk); g.drawCircle(p.x + b[0] * sc + (rnd(seed + i) - 0.5) * 2, p.y - 4 * sc - b[1] * sc, b[2] * sc); g.endFill(); });
    g.beginFill(S(M.leafLt, 1.05), 0.9); g.drawCircle(p.x - 3 * sc, p.y - 8 * sc, 3 * sc); g.endFill();
  }

  function olive(g, proj, gx, gy, z, sc) {
    sc = sc || 1; const p = proj.p(gx, gy, z || 0);
    g.beginFill(M.shadow, 0.18); g.drawEllipse(p.x + 5 * sc, p.y, 12 * sc, 4 * sc); g.endFill();
    g.lineStyle({ width: 2.4 * sc, color: M.woodDk }); g.moveTo(p.x, p.y); g.lineTo(p.x - 1 * sc, p.y - 11 * sc); g.lineStyle(0);
    [[0, -16, 11], [-7, -13, 7], [7, -14, 7], [0, -22, 8]].forEach((b, i) => { g.beginFill(i % 2 ? M.leaf : M.leafDk); g.drawCircle(p.x + b[0] * sc, p.y + b[1] * sc, b[2] * sc); g.endFill(); });
    g.beginFill(M.leafLt, 0.9); g.drawCircle(p.x - 4 * sc, p.y - 20 * sc, 4 * sc); g.endFill();
  }

  // low planted garden bed across a footprint area (world coords)
  function gardenBed(g, proj, x0, y0, w, d, seed) {
    seed = seed || 1;
    const a = proj.p(x0, y0, 0.02), b = proj.p(x0 + w, y0, 0.02), c = proj.p(x0 + w, y0 + d, 0.02), e = proj.p(x0, y0 + d, 0.02);
    ISO.poly(g, [a, b, c, e], M.grassDk);
    ISO.poly(g, [a, b, c, e], M.grass, 0.55);
    ISO.outlinePoly(g, [a, b, c, e], S(M.grassDk, 0.8), 1.4, 0.6);
    const cols = [M.flowerA, M.flowerB, M.flowerC];
    for (let i = 0; i < Math.round(w * d * 8); i++) {
      const u = rnd(seed + i), v = rnd(seed + i + 50);
      const pt = proj.p(x0 + 0.1 + u * (w - 0.2), y0 + 0.1 + v * (d - 0.2), 0.02);
      g.beginFill(cols[i % 3], 0.95); g.drawCircle(pt.x, pt.y, 1.5); g.endFill();
    }
  }

  function hedge(g, proj, gx0, gy0, gx1, gy1, n, z) {
    for (let i = 0; i <= n; i++) { const t = i / n; bush(g, proj, gx0 + (gx1 - gx0) * t, gy0 + (gy1 - gy0) * t, z || 0, 0.6, i * 7); }
  }

  function amphora(g, proj, gx, gy, z, sc, color) {
    sc = sc || 1; color = color || M.terracotta; const p = proj.p(gx, gy, z || 0);
    g.beginFill(M.shadow, 0.16); g.drawEllipse(p.x + 2 * sc, p.y, 4 * sc, 1.6 * sc); g.endFill();
    g.beginFill(S(color, 0.92)); g.drawEllipse(p.x, p.y - 7 * sc, 4 * sc, 7 * sc); g.endFill();
    g.beginFill(S(color, 1.12)); g.drawEllipse(p.x - 1.3 * sc, p.y - 8 * sc, 1.6 * sc, 4.5 * sc); g.endFill();
    g.beginFill(S(color, 0.78)); g.drawRect(p.x - 1.4 * sc, p.y - 15 * sc, 2.8 * sc, 4 * sc); g.endFill();
    g.lineStyle({ width: 1, color: S(color, 0.7) }); g.moveTo(p.x - 3.6 * sc, p.y - 13 * sc); g.lineTo(p.x - 2 * sc, p.y - 10 * sc); g.moveTo(p.x + 3.6 * sc, p.y - 13 * sc); g.lineTo(p.x + 2 * sc, p.y - 10 * sc); g.lineStyle(0);
  }

  function urn(g, proj, gx, gy, z, sc) {
    sc = sc || 1; const p = proj.p(gx, gy, z || 0);
    g.beginFill(S(M.marble, 0.9)); g.drawRect(p.x - 3 * sc, p.y - 4 * sc, 6 * sc, 4 * sc); g.endFill();
    g.beginFill(M.marble); g.drawEllipse(p.x, p.y - 10 * sc, 5 * sc, 6 * sc); g.endFill();
    g.beginFill(M.bush); g.drawCircle(p.x - 2 * sc, p.y - 15 * sc, 3 * sc); g.drawCircle(p.x + 2 * sc, p.y - 14 * sc, 3 * sc); g.drawCircle(p.x, p.y - 17 * sc, 3 * sc); g.endFill();
  }

  function statue(g, proj, gx, gy, z, sc, mat) {
    sc = sc || 1; mat = mat || M.marble; const p = proj.p(gx, gy, z || 0);
    g.beginFill(M.shadow, 0.16); g.drawEllipse(p.x + 3 * sc, p.y, 7 * sc, 2.4 * sc); g.endFill();
    g.beginFill(S(mat, 0.84)); g.drawRect(p.x - 4 * sc, p.y - 7 * sc, 8 * sc, 7 * sc); g.endFill();
    g.beginFill(S(mat, 1.0)); g.drawRect(p.x - 4.6 * sc, p.y - 8.4 * sc, 9.2 * sc, 1.6 * sc); g.endFill();
    // figure
    g.beginFill(S(mat, 1.08)); g.drawRect(p.x - 2.6 * sc, p.y - 22 * sc, 5.2 * sc, 14 * sc); g.endFill();
    g.beginFill(S(mat, 0.9)); g.drawRect(p.x + 0.4 * sc, p.y - 22 * sc, 2.2 * sc, 14 * sc); g.endFill();
    g.beginFill(S(mat, 1.12)); g.drawCircle(p.x, p.y - 24 * sc, 2.6 * sc); g.endFill();
  }

  function fountain(g, proj, gx, gy, z, sc) {
    sc = sc || 1; const p = proj.p(gx, gy, z || 0);
    g.beginFill(S(M.marble, 0.86)); g.drawEllipse(p.x, p.y, 13 * sc, 6.5 * sc); g.endFill();
    g.beginFill(M.water); g.drawEllipse(p.x, p.y - 1 * sc, 10 * sc, 4.8 * sc); g.endFill();
    g.beginFill(S(M.water, 1.3), 0.6); g.drawEllipse(p.x - 2 * sc, p.y - 2 * sc, 5 * sc, 2 * sc); g.endFill();
    g.beginFill(M.marble); g.drawRect(p.x - 1.4 * sc, p.y - 10 * sc, 2.8 * sc, 9 * sc); g.endFill();
    g.beginFill(S(M.marble, 1.1)); g.drawEllipse(p.x, p.y - 10 * sc, 4 * sc, 1.8 * sc); g.endFill();
  }

  // a paved path strip (road feel) along gy at gx
  function pavers(g, proj, x0, y0, w, d) {
    const a = proj.p(x0, y0, 0.015), b = proj.p(x0 + w, y0, 0.015), c = proj.p(x0 + w, y0 + d, 0.015), e = proj.p(x0, y0 + d, 0.015);
    ISO.poly(g, [a, b, c, e], M.stone);
    ISO.poly(g, [a, b, c, e], S(M.stone, 1.06), 0.5);
    for (let i = 1; i < Math.round(d * 2); i++) { const t = i / Math.round(d * 2); ISO.line(g, ISO.lerp(a, e, t), ISO.lerp(b, c, t), S(M.stone, 0.78), 1, 0.4); }
  }

  global.PROP = { cypress, bush, olive, gardenBed, hedge, amphora, urn, statue, fountain, pavers };
})(window);
