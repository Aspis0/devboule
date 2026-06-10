/* =========================================================================
   buildings_a.js — Procedural Greek buildings, part A (PixiJS v7)
   temple · house · fortress · tower · lighthouse · market · warehouse
   Each builder: fn(level 0..4, opt) -> { container, body, anims, foot:[W,D] }
   ========================================================================= */
(function (global) {
  'use strict';
  const M = ISO.MAT;
  global.BUILD = global.BUILD || {};
  const B = global.BUILD;

  // shared registry (display + accent) ------------------------------------
  global.BUILD_META = {
    order: ['temple', 'house', 'fortress', 'tower', 'lighthouse', 'market',
      'warehouse', 'workshop', 'conduit', 'baths', 'theater', 'harbor',
      'library', 'townhall', 'unknown'],
    info: {
      temple: { name: 'Naos', cat: 'sacro', accent: M.blue },
      house: { name: 'Oikos', cat: 'abitazione', accent: M.terracotta },
      fortress: { name: 'Phrourion', cat: 'militare', accent: M.red },
      tower: { name: 'Pyrgos', cat: 'militare', accent: M.red },
      lighthouse: { name: 'Pharos', cat: 'porto', accent: M.gold },
      market: { name: 'Agora', cat: 'civile', accent: M.red },
      warehouse: { name: 'Apotheke', cat: 'civile', accent: M.wood },
      workshop: { name: 'Ergasterion', cat: 'produzione', accent: M.ochre },
      conduit: { name: 'Hydragogeion', cat: 'infrastruttura', accent: M.blue },
      baths: { name: 'Balaneion', cat: 'civile', accent: M.water },
      theater: { name: 'Theatron', cat: 'cultura', accent: M.red },
      harbor: { name: 'Limen', cat: 'porto', accent: M.blue },
      library: { name: 'Bibliotheke', cat: 'cultura', accent: M.blue },
      townhall: { name: 'Bouleuterion', cat: 'civile', accent: M.gold },
      unknown: { name: 'Agnoston', cat: 'fallback', accent: 0x8A8478 }
    }
  };

  function setup(W, D, opt) {
    const proj = ISO.makeProj(W, D);
    const c = new PIXI.Container();
    const g = new PIXI.Graphics();
    c.addChild(g);
    const out = !!(opt && opt.outline);
    const TEX = {};
    TEX[M.marble] = 'marble'; TEX[M.marbleCool] = 'marble'; TEX[M.marbleWarm] = 'plaster';
    TEX[M.plaster] = 'plaster'; TEX[M.mud] = 'plaster';
    TEX[M.stone] = 'ashlar'; TEX[M.plinth] = 'ashlar'; TEX[M.plinthDk] = 'ashlar';
    TEX[M.wood] = 'wood'; TEX[M.woodLight] = 'wood';
    const bx = (x, y, z, w, d, h, col, o) => {
      o = o || {};
      const t = o.tex !== undefined ? o.tex : TEX[col];
      return ISO.box(g, proj, x, y, z, w, d, h, col, Object.assign({ outline: out }, o, { tex: t }));
    };
    return { proj, c, g, out, bx, anims: [] };
  }

  // ====================== TEMPLE ==========================================
  B.temple = function (L, opt) {
    const sizes = [[2, 3], [2, 3], [3, 4], [3, 5], [4, 6]][L];
    const [W, D] = sizes;
    const s = setup(W, D, opt); const { proj, g, c, out, bx, anims } = s;

    const nStep = [2, 2, 3, 3, 4][L], stepH = 0.13, inset = 0.16;
    const topZ = ISO.steps(g, proj, 0, 0, 0, W, D, nStep, stepH, inset, M.stone);
    const ins = inset * nStep;
    const ix = ins, iy = ins, iw = W - 2 * ins, id = D - 2 * ins;
    const colH = 1.45 + L * 0.16, colR = 0.11 + L * 0.004;
    const peripteral = L >= 2;
    const frontN = [2, 3, 4, 4, 6][L], sideN = [3, 3, 5, 6, 8][L];

    // back colonnade
    if (peripteral) ISO.colonnade(g, proj, ix, iy, ix + iw, iy, topZ, colH, colR, frontN, M.marble, { outline: out });
    // cella
    const cw = iw * 0.6, cd = id * 0.76, cx0 = ix + (iw - cw) / 2, cy0 = iy + (id - cd) / 2;
    bx(cx0, cy0, topZ, cw, cd, colH * 0.94, M.marbleWarm);
    ISO.panelLeft(g, proj, cx0 + cw * 0.36, cy0 + cd, topZ, cw * 0.28, colH * 0.6, ISO.shade(M.wood, 0.7)); // door
    // side colonnades
    if (peripteral) {
      ISO.colonnade(g, proj, ix, iy, ix, iy + id, topZ, colH, colR, sideN, M.marble, { outline: out });
      ISO.colonnade(g, proj, ix + iw, iy, ix + iw, iy + id, topZ, colH, colR, sideN, M.marble, { outline: out });
    }
    // entablature + frieze
    const ez = topZ + colH;
    bx(ix - 0.06, iy - 0.06, ez, iw + 0.12, id + 0.12, 0.16, M.marble);
    ISO.panelLeft(g, proj, ix - 0.06, iy + id + 0.06, ez + 0.03, iw + 0.12, 0.09, ISO.shade(M.blue, ISO.faceFactor('left')));
    ISO.panelRight(g, proj, ix + iw + 0.06, iy - 0.06, ez + 0.03, id + 0.12, 0.09, ISO.shade(M.red, ISO.faceFactor('right')));
    // roof
    ISO.gableRoof(g, proj, ix - 0.06, iy - 0.06, ez + 0.16, iw + 0.12, id + 0.12,
      0.5 + 0.12 * L, M.terracotta, { ridge: 'y', overhang: 0.2, tympanum: L >= 2 ? M.blue : undefined, outline: out });
    // front colonnade (closest)
    ISO.colonnade(g, proj, ix, iy + id, ix + iw, iy + id, topZ, colH, colR, frontN, M.marble, { outline: out });
    // acroteria (gold) at apex
    if (L >= 3) {
      const ap = proj.p(ix + iw / 2, iy + id + 0.2, ez + 0.16 + 0.5 + 0.12 * L);
      g.beginFill(M.gold); g.drawCircle(ap.x, ap.y - 2, 3.4); g.endFill();
    }
    // altar + sacred flame in front
    if (L >= 2) {
      const ax = W / 2 - 0.2, ay = D + 0.18;
      ISO.box(g, proj, ax, ay, 0, 0.4, 0.4, 0.32, M.stone, { outline: out });
      const ap = proj.p(ax + 0.2, ay + 0.2, 0.32);
      const fl = new ANIM.Flame(ap.x, ap.y, 1.1 + 0.14 * L); c.addChild(fl.node); anims.push(fl);
    }
    // sacred grove: cypresses flanking + votive urns
    PROP.cypress(g, proj, -0.35, D - 0.2, 0, 1.15);
    PROP.cypress(g, proj, W + 0.35, D - 0.2, 0, 1.15);
    if (L >= 1) { PROP.urn(g, proj, 0.25, D + 0.05, 0, 1); PROP.urn(g, proj, W - 0.25, D + 0.05, 0, 1); }
    if (L >= 3) { PROP.cypress(g, proj, -0.3, D - 1.3, 0, 0.95); PROP.cypress(g, proj, W + 0.3, D - 1.3, 0, 0.95); }
    return { container: c, body: g, anims, foot: [W, D] };
  };

  // ====================== HOUSE ===========================================
  B.house = function (L, opt) {
    const cfg = [
      { W: 1, D: 1 }, { W: 1, D: 1 }, { W: 2, D: 2 },
      { W: 2, D: 2 }, { W: 3, D: 3 }
    ][L];
    const { W, D } = cfg;
    const s = setup(W, D, opt); const { proj, g, c, out, bx, anims } = s;

    function smokeAt(gx, gy, z, sc) {
      const p = proj.p(gx, gy, z); const sm = new ANIM.Smoke(p.x, p.y, sc || 1);
      c.addChild(sm.node); anims.push(sm);
    }

    if (L === 0) {
      // capanna: round-ish mud hut + thatch hip roof
      bx(0.15, 0.15, 0, 0.7, 0.7, 0.55, M.mud);
      ISO.panelLeft(g, proj, 0.38, 0.85, 0, 0.24, 0.4, ISO.shade(M.wood, 0.65));
      ISO.hipRoof(g, proj, 0.05, 0.05, 0.55, 0.9, 0.9, 0.5, M.thatch, { overhang: 0.12, outline: out });
      smokeAt(0.5, 0.5, 1.05, 0.7);
    } else if (L === 1) {
      // casa piccola: mudbrick box + tile gable
      bx(0.12, 0.12, 0, 0.76, 0.76, 0.7, M.mud);
      ISO.panelLeft(g, proj, 0.4, 0.88, 0, 0.22, 0.46, ISO.shade(M.wood, 0.6));
      ISO.panelLeft(g, proj, 0.18, 0.88, 0.42, 0.16, 0.16, M.blue);
      ISO.hipRoof(g, proj, 0.08, 0.08, 0.7, 0.8, 0.8, 0.46, M.terracotta, { overhang: 0.14, outline: out });
      smokeAt(0.3, 0.3, 1.16, 0.8);
    } else if (L === 2) {
      // casa: plastered 2x2, tile roof, windows
      bx(0.1, 0.1, 0, 1.8, 1.8, 0.95, M.marbleWarm);
      ISO.panelLeft(g, proj, 0.75, 1.9, 0, 0.4, 0.62, ISO.shade(M.wood, 0.6));
      ISO.panelLeft(g, proj, 0.28, 1.9, 0.5, 0.3, 0.3, M.blue);
      ISO.panelLeft(g, proj, 1.28, 1.9, 0.5, 0.3, 0.3, M.blue);
      ISO.hipRoof(g, proj, 0.05, 0.05, 0.95, 1.9, 1.9, 0.66, M.terracotta, { overhang: 0.18, outline: out });
      smokeAt(0.4, 0.4, 1.6, 0.95);
    } else if (L === 3) {
      // insula: two storeys
      bx(0.1, 0.1, 0, 1.8, 1.8, 1.0, M.marbleWarm);
      bx(0.22, 0.22, 1.0, 1.56, 1.56, 0.9, M.marble);
      // balcony band
      ISO.panelLeft(g, proj, 0.1, 1.9, 0.98, 1.8, 0.12, M.red);
      ISO.panelLeft(g, proj, 0.4, 1.9, 0.1, 0.4, 0.62, ISO.shade(M.wood, 0.6));
      [0.45, 1.05].forEach(x => ISO.panelLeft(g, proj, x, 1.9, 0.4, 0.3, 0.34, M.blue));
      [0.5, 1.05].forEach(x => ISO.panelLeft(g, proj, x, 1.78, 1.32, 0.28, 0.34, M.blueDeep));
      ISO.gableRoof(g, proj, 0.18, 0.18, 1.9, 1.64, 1.64, 0.5, M.terracotta, { ridge: 'y', overhang: 0.14, outline: out });
      smokeAt(0.5, 0.5, 2.5, 0.9);
    } else {
      // palazzo with courtyard (ring of rooms)
      const wings = [
        [0.1, 0.1, 2.8, 0.7], [0.1, 2.2, 2.8, 0.7],
        [0.1, 0.8, 0.7, 1.4], [2.2, 0.8, 0.7, 1.4]
      ];
      // courtyard floor
      ISO.box(g, proj, 0.8, 0.8, 0, 1.4, 1.4, 0.04, ISO.shade(M.stone, 1.04), { outline: out });
      // small inner colonnade
      ISO.colonnade(g, proj, 0.95, 2.05, 2.05, 2.05, 0.04, 0.7, 0.07, 4, M.marble, { outline: out });
      wings.forEach(w => {
        bx(w[0], w[1], 0, w[2], w[3], 1.05, M.marble);
        ISO.gableRoof(g, proj, w[0], w[1], 1.05, w[2], w[3], 0.38,
          M.terracotta, { ridge: w[2] > w[3] ? 'x' : 'y', overhang: 0.12, outline: out });
      });
      ISO.panelLeft(g, proj, 1.2, 3.0, 0, 0.6, 0.66, ISO.shade(M.wood, 0.55));
      smokeAt(0.45, 0.45, 1.7, 0.9);
    }
    // dooryard greenery (Caesar-style lived-in plots)
    if (L >= 1) { PROP.bush(g, proj, 0.05, D - 0.05, 0, 0.7, 11); PROP.bush(g, proj, W - 0.05, D + 0.02, 0, 0.7, 23); }
    if (L === 2 || L === 3) { PROP.olive(g, proj, W + 0.18, D - 0.4, 0, 0.8); PROP.gardenBed(g, proj, 0.0, D - 0.02, W * 0.5, 0.22, 5); }
    if (L === 4) {
      PROP.cypress(g, proj, 1.5, 1.5, 0.04, 0.9); PROP.cypress(g, proj, 1.5, 1.5, 0.04, 0.9);
      PROP.gardenBed(g, proj, 0.85, 0.85, 1.3, 1.3, 7);
      PROP.urn(g, proj, 0.9, 2.05, 0.04, 0.8); PROP.urn(g, proj, 2.05, 2.05, 0.04, 0.8);
      PROP.olive(g, proj, W + 0.2, D - 0.5, 0, 0.85); PROP.bush(g, proj, -0.05, D - 0.6, 0, 0.7, 3);
    }
    return { container: c, body: g, anims, foot: [W, D] };
  };

  // ====================== FORTRESS ========================================
  B.fortress = function (L, opt) {
    const sizes = [[2, 2], [2, 2], [3, 3], [3, 4], [4, 4]][L];
    const [W, D] = sizes;
    const s = setup(W, D, opt); const { proj, g, c, out, bx, anims } = s;
    const wallH = 1.0 + L * 0.1, towerH = wallH + 0.7 + L * 0.12;

    // base plinth
    bx(0, 0, 0, W, D, 0.18, M.plinth);
    const z0 = 0.18;
    const wallT = 0.28; // wall thickness
    // perimeter walls (4 boxes)
    bx(0, 0, z0, W, wallT, wallH, M.stone);                 // back
    bx(0, D - wallT, z0, W, wallT, wallH, M.stone);         // front
    bx(0, 0, z0, wallT, D, wallH, M.stone);                 // left
    bx(W - wallT, 0, z0, wallT, D, wallH, M.stone);         // right
    // merlons (crenellations) along front + right
    const merl = (gx, gy, n, axis) => {
      for (let i = 0; i < n; i++) {
        const t = (i + 0.5) / n;
        const mx = axis === 'x' ? gx + (W) * t - 0.12 : gx;
        const my = axis === 'y' ? gy + (D) * t - 0.12 : gy;
        bx(mx, my, z0 + wallH, 0.22, 0.22, 0.22, M.stone);
      }
    };
    merl(0, D - wallT + 0.03, Math.round(W * 2), 'x');
    merl(W - wallT + 0.03, 0, Math.round(D * 2), 'y');
    // keep (central tower)
    const kw = Math.max(0.9, W * 0.5), kd = Math.max(0.9, D * 0.5);
    const kx = (W - kw) / 2, ky = (D - kd) / 2;
    if (L >= 1) bx(kx, ky, z0, kw, kd, towerH, M.stone);
    // corner towers
    if (L >= 2) {
      const ct = 0.55, ch = towerH + 0.2;
      [[0, 0], [W - ct, 0], [0, D - ct], [W - ct, D - ct]].forEach(p =>
        bx(p[0], p[1], z0, ct, ct, ch, ISO.shade(M.stone, 1.02)));
    }
    // gate
    ISO.panelLeft(g, proj, W / 2 - 0.3, D, z0, 0.6, wallH * 0.7, ISO.shade(M.wood, 0.5));
    // banner on keep
    const fp = proj.p(kx + kw / 2, ky + kd / 2, z0 + towerH);
    const fg = new ANIM.Flag(fp.x, fp.y, 1.1 + 0.06 * L, M.red);
    c.addChild(fg.node); anims.push(fg);
    return { container: c, body: g, anims, foot: [W, D] };
  };

  // ====================== TOWER ===========================================
  B.tower = function (L, opt) {
    const sizes = [[1, 1], [1, 1], [1, 1], [2, 2], [2, 2]][L];
    const [W, D] = sizes;
    const s = setup(W, D, opt); const { proj, g, c, out, bx, anims } = s;
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
    bx(inset - 0.08, inset - 0.08, 0.16 + h, W - 2 * inset + 0.16, D - 2 * inset + 0.16, 0.26, ISO.shade(M.stone, 1.03));
    const topZ = 0.16 + h + 0.26;
    const n = Math.round((W - 2 * inset) * 3);
    for (let i = 0; i < n; i++) {
      const t = (i + 0.5) / n;
      bx(inset - 0.08 + (W - 2 * inset + 0.16) * t - 0.08, D - inset + 0.0, topZ, 0.16, 0.16, 0.2, M.stone);
    }
    // arrow-slit windows
    [0.4, 1.1, 1.8, 2.5].filter(z => z < h - 0.3).forEach(z =>
      ISO.panelLeft(g, proj, W / 2 - 0.06, D - inset, 0.16 + z, 0.12, 0.32, ISO.shade(M.ink, 1.4)));
    if (L >= 3) {
      const fp = proj.p(W / 2, D / 2, topZ + 0.2);
      const fg = new ANIM.Flag(fp.x, fp.y, 0.95, M.red); c.addChild(fg.node); anims.push(fg);
    }
    return { container: c, body: g, anims, foot: [W, D] };
  };

  // ====================== LIGHTHOUSE (Pharos) =============================
  B.lighthouse = function (L, opt) {
    const [W, D] = [2, 2];
    const s = setup(W, D, opt); const { proj, g, c, out, bx, anims } = s;
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
      ISO.cylinder(g, proj, 1.0, 1.0, z, 0.5, midH, M.marbleWarm, { seg: 8, outline: out });
      z += midH;
    }
    // round drum (lantern housing)
    ISO.cylinder(g, proj, 1.0, 1.0, z, 0.36, drumH, M.marble, { seg: 16, outline: out });
    z += drumH;
    // little colonnade ring around lantern
    if (L >= 2) ISO.colonnade(g, proj, 0.7, 1.0, 1.3, 1.0, z - drumH, drumH * 0.8, 0.05, 3, M.marble, {});
    // cap + statue
    bx(0.78, 0.78, z, 0.44, 0.44, 0.18, ISO.shade(M.bronze, 1.0));
    if (L >= 4) { const sp = proj.p(1.0, 1.0, z + 0.18); g.beginFill(M.gold); g.drawCircle(sp.x, sp.y - 8, 4); g.endFill(); }
    // beacon fire at top
    const bp = proj.p(1.0, 1.0, z + 0.2);
    const bc = new ANIM.Beacon(bp.x, bp.y, 1.0 + 0.08 * L); c.addChild(bc.node); anims.push(bc);
    return { container: c, body: g, anims, foot: [W, D] };
  };

  // ====================== MARKET (agora stalls + stoa) ====================
  B.market = function (L, opt) {
    const sizes = [[2, 2], [2, 3], [3, 3], [3, 4], [4, 4]][L];
    const [W, D] = sizes;
    const s = setup(W, D, opt); const { proj, g, c, out, bx, anims } = s;
    // paved plot
    ISO.box(g, proj, 0, 0, 0, W, D, 0.05, ISO.shade(M.stone, 1.05), { outline: out });
    // back stoa: colonnade + roof along the back edge
    const stoaD = 0.7;
    bx(0, 0, 0.05, W, stoaD, 0.1, M.marble);
    ISO.colonnade(g, proj, 0.2, stoaD, W - 0.2, stoaD, 0.15, 0.95, 0.08, Math.max(3, W + 1), M.marble, { outline: out });
    bx(0, 0, 0.05 + 1.1, W, stoaD, 0.16, M.marble);
    ISO.gableRoof(g, proj, 0, 0, 0.05 + 1.26, W, stoaD, 0.34, M.terracotta, { ridge: 'x', overhang: 0.14, outline: out });
    // market stalls with striped awnings
    const stalls = [
      [0.4, 1.2], [1.5, 1.2], [0.5, 2.1], [1.7, 2.2], [2.6, 1.4], [2.7, 2.5], [0.6, 3.0], [1.9, 3.1]
    ].filter(p => p[0] < W - 0.3 && p[1] < D - 0.2).slice(0, [2, 3, 4, 6, 8][L]);
    const acc = [M.red, M.blue];
    stalls.forEach((p, i) => {
      bx(p[0], p[1], 0.05, 0.5, 0.5, 0.45, M.wood);
      // awning: striped quad on top, slightly larger
      const a = proj.p(p[0] - 0.1, p[1] - 0.1, 0.62), b = proj.p(p[0] + 0.65, p[1] - 0.1, 0.62);
      const cc = proj.p(p[0] + 0.65, p[1] + 0.6, 0.55), d = proj.p(p[0] - 0.1, p[1] + 0.6, 0.55);
      ISO.poly(g, [a, b, cc, d], acc[i % 2]);
      ISO.poly(g, [a, proj.p(p[0] + 0.27, p[1] - 0.1, 0.62), proj.p(p[0] + 0.27, p[1] + 0.6, 0.55), d], ISO.shade(M.marble, 1.05));
      // amphorae
      const ap = proj.p(p[0] + 0.6, p[1] + 0.55, 0.05);
      g.beginFill(ISO.shade(M.terracotta, 0.9)); g.drawEllipse(ap.x, ap.y - 5, 4, 7); g.endFill();
    });
    PROP.cypress(g, proj, -0.25, D - 0.3, 0, 0.85);
    PROP.cypress(g, proj, W + 0.25, D - 0.3, 0, 0.85);
    PROP.amphora(g, proj, 0.2, D - 0.1, 0.05, 1, M.ochre);
    PROP.amphora(g, proj, 0.45, D - 0.15, 0.05, 0.9, M.terracotta);
    return { container: c, body: g, anims, foot: [W, D] };
  };

  // ====================== WAREHOUSE =======================================
  B.warehouse = function (L, opt) {
    const sizes = [[2, 2], [2, 3], [3, 3], [4, 3], [4, 4]][L];
    const [W, D] = sizes;
    const s = setup(W, D, opt); const { proj, g, c, out, bx, anims } = s;
    ISO.box(g, proj, 0, 0, 0, W, D, 0.08, M.plinth, { outline: out });
    // central roofed depot
    const dw = W - 0.4, dd = Math.min(D - 0.4, 1.6);
    bx(0.2, 0.2, 0.08, dw, dd, 0.9, M.marbleWarm);
    ISO.gableRoof(g, proj, 0.15, 0.15, 0.98, dw + 0.1, dd + 0.1, 0.5, M.terracotta, { ridge: 'x', overhang: 0.16, outline: out });
    // open storage bays in front: amphora/sack stacks under little awnings
    const slotsY = D - 0.7;
    const cols = Math.round(W);
    for (let i = 0; i < cols; i++) {
      const x = 0.25 + i * (W - 0.5) / Math.max(1, cols - 1 || 1);
      // wooden frame
      bx(x, slotsY, 0.08, 0.12, 0.5, 0.6, M.wood);
      // goods
      const gp = proj.p(x + 0.32, slotsY + 0.4, 0.08);
      for (let k = 0; k < 3; k++) {
        g.beginFill(ISO.shade(k % 2 ? M.terracotta : M.ochre, 0.95 - k * 0.08));
        g.drawEllipse(gp.x + (k - 1) * 6, gp.y - 6 - k * 2, 4.5, 7); g.endFill();
      }
    }
    // depot flag
    if (L >= 2) {
      const fp = proj.p(0.4, 0.4, 0.98 + 0.5 + 0.3);
      const fg = new ANIM.Flag(fp.x, fp.y, 0.8, M.ochre); c.addChild(fg.node); anims.push(fg);
    }
    return { container: c, body: g, anims, foot: [W, D] };
  };

})(window);
