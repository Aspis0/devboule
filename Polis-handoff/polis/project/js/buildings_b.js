/* =========================================================================
   buildings_b.js — Procedural Greek buildings, part B (PixiJS v7)
   workshop · conduit · baths · theater · harbor · library · townhall · unknown
   ========================================================================= */
(function (global) {
  'use strict';
  const M = ISO.MAT;
  global.BUILD = global.BUILD || {};
  const B = global.BUILD;

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

  // ====================== WORKSHOP ========================================
  B.workshop = function (L, opt) {
    const sizes = [[1, 1], [2, 2], [2, 2], [3, 2], [3, 3]][L];
    const [W, D] = sizes;
    const s = setup(W, D, opt); const { proj, g, c, out, bx, anims } = s;
    ISO.box(g, proj, 0, 0, 0, W, D, 0.07, M.plinth, { outline: out });
    // shed
    const sw = W - 0.3, sd = D - 0.5;
    bx(0.15, 0.15, 0.07, sw, sd, 0.85, M.mud);
    ISO.gableRoof(g, proj, 0.1, 0.1, 0.92, sw + 0.1, sd + 0.1, 0.42, M.terracotta, { ridge: 'x', overhang: 0.14, outline: out });
    // kiln / furnace (stone, glowing mouth) in front-right, with chimney
    const nKiln = [1, 1, 2, 2, 3][L];
    for (let i = 0; i < nKiln; i++) {
      const kx = 0.25 + i * 0.85, ky = D - 0.55;
      if (kx > W - 0.4) break;
      bx(kx, ky, 0.07, 0.5, 0.45, 0.7, M.stone);
      // glowing mouth
      ISO.panelLeft(g, proj, kx + 0.12, ky + 0.45, 0.12, 0.26, 0.22, 0xE2761F);
      // chimney + smoke
      bx(kx + 0.16, ky + 0.1, 0.77, 0.2, 0.2, 0.4, ISO.shade(M.stone, 0.9));
      const sp = proj.p(kx + 0.26, ky + 0.2, 1.17);
      const sm = new ANIM.Smoke(sp.x, sp.y, 0.85); c.addChild(sm.node); anims.push(sm);
      // small flame at mouth
      const fp = proj.p(kx + 0.25, ky + 0.46, 0.16);
      const fl = new ANIM.Flame(fp.x, fp.y, 0.55); c.addChild(fl.node); anims.push(fl);
    }
    // wood pile
    const wp = proj.p(0.3, D - 0.3, 0.07);
    for (let k = 0; k < 3; k++) { g.beginFill(ISO.shade(M.wood, 1 - k * 0.06)); g.drawEllipse(wp.x - 6 + k * 5, wp.y - 3 - k * 3, 7, 3); g.endFill(); }
    return { container: c, body: g, anims, foot: [W, D] };
  };

  // ====================== CONDUIT (aqueduct arcade) =======================
  B.conduit = function (L, opt) {
    const len = [2, 3, 3, 4, 5][L];     // spans along gy
    const [W, D] = [1, len];
    const s = setup(W, D, opt); const { proj, g, c, out, bx, anims } = s;
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
      const a = proj.p(0.36, i + pierW / 2, 0.1), b = proj.p(0.36, i + 1 - pierW / 2, 0.1);
      const cc = proj.p(0.36, i + 0.5, pierH - 0.05);
      ISO.poly(g, [a, b, cc], ISO.shade(M.ink, 1.6), 0.85);
    }
    // top channel box
    bx(0.3, -0.05, pierH, 0.42, len + 0.1, 0.5, M.marbleWarm);
    // water surface along the channel (animated)
    const wy0 = 0.02, wy1 = len + 0.02, wz = pierH + 0.42, wx0 = 0.36, wx1 = 0.66;
    const wpts = [proj.p(wx0, wy0, wz), proj.p(wx1, wy0, wz), proj.p(wx1, wy1, wz), proj.p(wx0, wy1, wz)];
    const w = new ANIM.Water(wpts, 0.85); c.addChild(w.node); anims.push(w);
    void channelTop;
    return { container: c, body: g, anims, foot: [W, D] };
  };

  // ====================== BATHS ===========================================
  B.baths = function (L, opt) {
    const sizes = [[2, 2], [2, 3], [3, 3], [3, 4], [4, 4]][L];
    const [W, D] = sizes;
    const s = setup(W, D, opt); const { proj, g, c, out, bx, anims } = s;
    ISO.box(g, proj, 0, 0, 0, W, D, 0.1, M.stone, { outline: out });
    // enclosed hall at back
    const hd = Math.min(1.4, D * 0.5);
    bx(0.1, 0.0, 0.1, W - 0.2, hd, 1.0, M.marbleWarm);
    ISO.gableRoof(g, proj, 0.05, -0.05, 1.1, W - 0.1, hd + 0.05, 0.5, M.terracotta, { ridge: 'x', overhang: 0.16, tympanum: M.blue, outline: out });
    // colonnade framing the pool (front)
    const poolY0 = hd + 0.15, poolY1 = D - 0.25;
    ISO.colonnade(g, proj, 0.25, poolY0, W - 0.25, poolY0, 0.1, 0.9, 0.07, Math.max(3, W), M.marble, { outline: out });
    if (L >= 2) {
      ISO.colonnade(g, proj, 0.25, poolY1 + 0.05, W - 0.25, poolY1 + 0.05, 0.1, 0.9, 0.07, Math.max(3, W), M.marble, { outline: out });
    }
    // sunken pool with water
    const px0 = 0.35, px1 = W - 0.35, pz = 0.06;
    const wpts = [proj.p(px0, poolY0 + 0.08, pz), proj.p(px1, poolY0 + 0.08, pz), proj.p(px1, poolY1, pz), proj.p(px0, poolY1, pz)];
    // pool rim
    ISO.poly(g, [proj.p(px0 - 0.08, poolY0, 0.1), proj.p(px1 + 0.08, poolY0, 0.1), proj.p(px1 + 0.08, poolY1 + 0.08, 0.1), proj.p(px0 - 0.08, poolY1 + 0.08, 0.1)], ISO.shade(M.marble, 0.9));
    const w = new ANIM.Water(wpts, 0.9); c.addChild(w.node); anims.push(w);
    PROP.urn(g, proj, 0.2, D - 0.1, 0.1, 1); PROP.urn(g, proj, W - 0.2, D - 0.1, 0.1, 1);
    PROP.cypress(g, proj, -0.3, D - 0.5, 0, 0.95); PROP.cypress(g, proj, W + 0.3, D - 0.5, 0, 0.95);
    return { container: c, body: g, anims, foot: [W, D] };
  };

  // ====================== THEATER (cavea + skene) =========================
  B.theater = function (L, opt) {
    const sizes = [[3, 2], [3, 3], [4, 3], [4, 4], [5, 4]][L];
    const [W, D] = sizes;
    const s = setup(W, D, opt); const { proj, g, c, out } = s;
    const cx = W / 2, cy = 0.2;          // centre of the arcs (near back)
    const tiers = [3, 4, 5, 6, 7][L];
    const rMax = Math.min(W / 2 + 0.2, D - 0.4);
    const seatH = 0.16;
    // draw concentric seating rings from outer(back/top) to inner(front)
    for (let ti = tiers - 1; ti >= 0; ti--) {
      const r = 0.6 + (rMax - 0.6) * (ti / (tiers - 1));
      const z = ti * seatH;
      const seg = 22;
      // front-facing half-annulus (angles ~ from 0.15π to 0.85π => facing camera/front)
      const a0 = Math.PI * 0.12, a1 = Math.PI * 0.88;
      const outer = [], inner = [];
      for (let i = 0; i <= seg; i++) {
        const a = a0 + (a1 - a0) * (i / seg);
        outer.push(proj.p(cx + Math.cos(a) * r, cy + Math.sin(a) * r, z));
        inner.push(proj.p(cx + Math.cos(a) * (r - 0.34), cy + Math.sin(a) * (r - 0.34), z));
      }
      // riser (vertical front of the step)
      const riser = [];
      for (let i = 0; i <= seg; i++) {
        const a = a0 + (a1 - a0) * (i / seg);
        riser.push(proj.p(cx + Math.cos(a) * r, cy + Math.sin(a) * r, z - seatH));
      }
      // step top
      const top = inner.concat(outer.slice().reverse());
      const topFlat = []; top.forEach(p => topFlat.push(p.x, p.y));
      g.beginFill(ISO.shade(M.stone, 1.08)); g.drawPolygon(topFlat); g.endFill();
      // riser face
      const rf = outer.concat(riser.slice().reverse());
      const rfFlat = []; rf.forEach(p => rfFlat.push(p.x, p.y));
      g.beginFill(ISO.shade(M.stone, 0.74)); g.drawPolygon(rfFlat); g.endFill();
      if (out) { g.lineStyle({ width: 1, color: M.ink, alpha: 0.2 }); g.drawPolygon(topFlat); g.lineStyle(0); }
    }
    // orchestra (round stage floor)
    const op = []; for (let i = 0; i <= 24; i++) { const a = i / 24 * Math.PI * 2; op.push(proj.p(cx + Math.cos(a) * 0.5, cy + Math.sin(a) * 0.5, 0.01)); }
    const opF = []; op.forEach(p => opF.push(p.x, p.y));
    g.beginFill(ISO.shade(M.stone, 1.14)); g.drawPolygon(opF); g.endFill();
    // skene (stage building) at the very front
    ISO.box(g, proj, 0.4, D - 0.55, 0, W - 0.8, 0.4, 0.95, M.marbleWarm, { outline: out });
    ISO.colonnade(g, proj, 0.6, D - 0.15, W - 0.6, D - 0.15, 0.0, 0.85, 0.07, Math.max(3, W), M.marble, { outline: out });
    ISO.box(g, proj, 0.35, D - 0.6, 0.95, W - 0.7, 0.5, 0.16, M.marble, { outline: out });
    PROP.cypress(g, proj, -0.3, D - 0.6, 0, 1.0); PROP.cypress(g, proj, W + 0.3, D - 0.6, 0, 1.0);
    PROP.statue(g, proj, 0.5, D - 0.05, 0, 0.8); PROP.statue(g, proj, W - 0.5, D - 0.05, 0, 0.8);
    return { container: c, body: g, anims: s.anims, foot: [W, D] };
  };

  // ====================== HARBOR ==========================================
  B.harbor = function (L, opt) {
    const sizes = [[2, 2], [3, 2], [3, 3], [4, 3], [4, 4]][L];
    const [W, D] = sizes;
    const s = setup(W, D, opt); const { proj, g, c, out, bx, anims } = s;
    // water fills the whole plot; quay sits at the back
    const wpts = [proj.p(0, 0, 0), proj.p(W, 0, 0), proj.p(W, D, 0), proj.p(0, D, 0)];
    const w = new ANIM.Water(wpts, 0.95); c.addChildAt(w.node, 0); anims.push(w);
    // stone quay (back strip)
    const quayD = 0.9;
    bx(0, 0, 0, W, quayD, 0.3, M.stone);
    // harbor master house on the quay
    bx(0.2, 0.05, 0.3, 1.0, 0.6, 0.85, M.marbleWarm);
    ISO.gableRoof(g, proj, 0.15, 0.0, 1.15, 1.1, 0.65, 0.4, M.terracotta, { ridge: 'x', overhang: 0.12, outline: out });
    // wooden piers extending into the water (front)
    const piers = [1, 1, 2, 2, 3][L];
    for (let i = 0; i < piers; i++) {
      const px = 0.4 + i * (W - 0.8) / Math.max(1, piers);
      bx(px, quayD, 0.18, 0.3, D - quayD - 0.2, 0.12, M.wood);
      // bollards
      [quayD + 0.2, D - 0.4].forEach(py => bx(px - 0.04, py, 0.3, 0.12, 0.12, 0.18, M.woodLight));
    }
    // crane (wooden A-frame) on the quay
    if (L >= 2) {
      const cb = proj.p(W - 0.6, quayD, 0.3), ct = proj.p(W - 0.9, quayD + 0.6, 1.5);
      g.lineStyle({ width: 4, color: M.wood }); g.moveTo(cb.x, cb.y); g.lineTo(ct.x, ct.y);
      const arm = proj.p(W - 0.9, D - 0.3, 1.5); g.lineTo(arm.x, arm.y); g.lineStyle(0);
      g.beginFill(M.wood); g.drawCircle(ct.x, ct.y, 3); g.endFill();
      g.lineStyle({ width: 1.5, color: M.ink, alpha: 0.6 }); g.moveTo(arm.x, arm.y); g.lineTo(arm.x, arm.y + 14); g.lineStyle(0);
    }
    PROP.amphora(g, proj, 1.4, 0.25, 0.3, 1, M.ochre);
    PROP.amphora(g, proj, 1.62, 0.3, 0.3, 0.9, M.terracotta);
    PROP.amphora(g, proj, 1.5, 0.5, 0.3, 0.95, M.terracotta);
    return { container: c, body: g, anims, foot: [W, D] };
  };

  // ====================== LIBRARY (two-storey Ionic stoa) =================
  B.library = function (L, opt) {
    const sizes = [[2, 2], [3, 2], [3, 3], [4, 3], [4, 3]][L];
    const [W, D] = sizes;
    const s = setup(W, D, opt); const { proj, g, c, out, bx, anims } = s;
    const z0 = ISO.steps(g, proj, 0, 0, 0, W, D, 2, 0.12, 0.1, M.stone);
    const ins = 0.2;
    // main block
    bx(ins, ins, z0, W - 2 * ins, D - 2 * ins, 1.7, M.marble);
    // niches (scroll shelves) on front
    for (let i = 0; i < Math.round(W); i++) {
      const x = ins + 0.25 + i * (W - 2 * ins - 0.3) / Math.max(1, Math.round(W) - 1 || 1);
      ISO.panelLeft(g, proj, x, D - ins, z0 + 0.3, 0.3, 0.5, ISO.shade(M.ink, 1.5));
      ISO.panelLeft(g, proj, x, D - ins, z0 + 1.0, 0.3, 0.5, ISO.shade(M.ink, 1.5));
    }
    // two-storey Ionic colonnade across the front porch
    ISO.colonnade(g, proj, ins + 0.1, D - 0.1, W - ins - 0.1, D - 0.1, z0, 1.6, 0.1, Math.max(4, W + 1), M.marble, { ionic: true, outline: out });
    // entablature + roof
    bx(ins - 0.05, ins - 0.05, z0 + 1.7, W - 2 * ins + 0.1, D - 2 * ins + 0.1, 0.18, M.marble);
    ISO.panelLeft(g, proj, ins - 0.05, D - ins + 0.05, z0 + 1.74, W - 2 * ins + 0.1, 0.1, ISO.shade(M.blue, ISO.faceFactor('left')));
    ISO.gableRoof(g, proj, ins - 0.05, ins - 0.05, z0 + 1.88, W - 2 * ins + 0.1, D - 2 * ins + 0.1, 0.5, M.terracotta, { ridge: 'y', overhang: 0.18, tympanum: M.red, outline: out });
    PROP.statue(g, proj, 0.4, D - 0.05, z0, 0.8); PROP.statue(g, proj, W - 0.4, D - 0.05, z0, 0.8);
    PROP.cypress(g, proj, -0.3, D - 0.5, 0, 0.95); PROP.cypress(g, proj, W + 0.3, D - 0.5, 0, 0.95);
    return { container: c, body: g, anims, foot: [W, D] };
  };

  // ====================== TOWNHALL (bouleuterion) =========================
  B.townhall = function (L, opt) {
    const sizes = [[2, 2], [3, 3], [3, 3], [4, 4], [4, 5]][L];
    const [W, D] = sizes;
    const s = setup(W, D, opt); const { proj, g, c, out, bx, anims } = s;
    const z0 = ISO.steps(g, proj, 0, 0, 0, W, D, 2, 0.13, 0.1, M.stone);
    const ins = 0.25;
    const bodyH = 1.2 + L * 0.12;
    // main hall
    bx(ins, ins, z0, W - 2 * ins, D - 2 * ins - 0.4, bodyH, M.marbleWarm);
    // hipped roof
    ISO.hipRoof(g, proj, ins - 0.05, ins - 0.05, z0 + bodyH, W - 2 * ins + 0.1, D - 2 * ins - 0.4 + 0.1, 0.7 + 0.1 * L, M.terracotta, { overhang: 0.18, outline: out });
    // front porch colonnade + small pediment
    const porchY = D - ins - 0.1;
    ISO.colonnade(g, proj, ins + 0.15, porchY, W - ins - 0.15, porchY, z0, 1.0, 0.09, Math.max(3, W), M.marble, { outline: out });
    bx(ins + 0.05, porchY - 0.05, z0 + 1.0, W - 2 * ins - 0.1, 0.32, 0.16, M.marble);
    ISO.gableRoof(g, proj, ins + 0.05, porchY - 0.1, z0 + 1.16, W - 2 * ins - 0.1, 0.42, 0.34, M.terracotta, { ridge: 'x', overhang: 0.12, pediment: M.marble, tympanum: M.gold, outline: out });
    // door
    ISO.panelLeft(g, proj, W / 2 - 0.25, D - ins - 0.4, z0, 0.5, bodyH * 0.6, ISO.shade(M.bronze, 0.85));
    // civic banner on the ridge
    const fp = proj.p(W / 2, (ins + (D - 0.4)) / 2, z0 + bodyH + 0.7 + 0.1 * L);
    const fg = new ANIM.Flag(fp.x, fp.y, 1.1, M.gold); c.addChild(fg.node); anims.push(fg);
    PROP.statue(g, proj, 0.45, D - 0.05, z0, 0.85); PROP.statue(g, proj, W - 0.45, D - 0.05, z0, 0.85);
    PROP.urn(g, proj, 0.2, D + 0.05, 0, 0.9); PROP.urn(g, proj, W - 0.2, D + 0.05, 0, 0.9);
    return { container: c, body: g, anims, foot: [W, D] };
  };

  // ====================== UNKNOWN (fallback) ==============================
  B.unknown = function (L, opt) {
    const [W, D] = [1, 1];
    const s = setup(W, D, opt); const { proj, g, c, out, bx } = s;
    // striped placeholder plinth
    ISO.box(g, proj, 0, 0, 0, 1, 1, 0.1, ISO.shade(M.stone, 0.96), { outline: true });
    // hatched top
    for (let i = -2; i < 6; i++) {
      const a = proj.p(i * 0.18, 0, 0.1), b = proj.p(i * 0.18 + 0.9, 0.9, 0.1);
      g.lineStyle({ width: 3, color: ISO.shade(M.groundEdge, 1.0), alpha: 0.5 });
      g.moveTo(a.x, a.y); g.lineTo(b.x, b.y);
    }
    g.lineStyle(0);
    // crate
    bx(0.18, 0.18, 0.1, 0.64, 0.64, 0.7, M.wood, { outline: true });
    // big "?" mark
    const qp = proj.p(0.5, 0.5, 0.82);
    const t = new PIXI.Text('?', { fontFamily: 'Georgia, serif', fontSize: 34, fill: 0xF4EFE6, fontWeight: '700' });
    t.anchor.set(0.5, 1); t.position.set(qp.x, qp.y - 2); c.addChild(t);
    void L; void out;
    return { container: c, body: g, anims: s.anims, foot: [W, D] };
  };

})(window);
