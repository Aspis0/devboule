/* =========================================================================
   figures.js — Monumental sculpted figures (PixiJS v7)
   -------------------------------------------------------------------------
   Screen-space "billboard" figures drawn at a base point (the feet/plinth
   top), extending upward (−y). Same primitive language as PROP.statue but
   bigger and characterful: heroic kouros (Colossus), enthroned deity
   (Zeus), standing goddess (Athena Parthenos), caryatid maiden, quadriga,
   winged Victory. Materials follow ISO.MAT; a left-lit (NW sun) read is
   baked in (lit on the figure's right side = viewer left).
   Each fn(g, x, y, sc, opt) draws into Graphics g. Heights ≈ 90·sc px.
   ========================================================================= */
(function (global) {
  'use strict';
  const M = ISO.MAT, S = ISO.shade;

  function tone(mat) {
    return {
      hl: S(mat, 1.2), lit: S(mat, 1.08), mid: S(mat, 0.95),
      dk: S(mat, 0.74), dk2: S(mat, 0.6)
    };
  }
  function shadow(g, x, y, sc, w) {
    g.beginFill(M.shadow, 0.18); g.drawEllipse(x + 3 * sc, y, (w || 11) * sc, 3.4 * sc); g.endFill();
  }
  // a tapered limb/trunk as a 4-pt poly between two widths
  function taper(g, x0, y0, w0, x1, y1, w1, col) {
    g.beginFill(col);
    g.drawPolygon([x0 - w0, y0, x0 + w0, y0, x1 + w1, y1, x1 - w1, y1]);
    g.endFill();
  }

  // ---- heroic standing male (Colossus of Rhodes / Helios) ---------------
  function heroicMale(g, x, y, sc, opt) {
    opt = opt || {}; const mat = opt.mat || M.bronze; const t = tone(mat);
    shadow(g, x, y, sc, 13);
    const hipY = y - 38 * sc, shoY = y - 64 * sc, headY = y - 76 * sc;
    // legs (slight stance) — right (viewer-left) lit, left in shade
    taper(g, x - 6 * sc, y, 4.6 * sc, x - 4 * sc, hipY, 4 * sc, t.lit);
    taper(g, x + 6 * sc, y, 4.6 * sc, x + 4 * sc, hipY, 4 * sc, t.dk);
    // shin highlights
    g.beginFill(t.hl, 0.5); g.drawRect(x - 9 * sc, hipY + 4 * sc, 2 * sc, (y - hipY) - 6 * sc); g.endFill();
    // hip wrap / short himation
    const cloth = opt.cloth || mat;
    g.beginFill(S(cloth, 0.9)); g.drawPolygon([x - 9 * sc, hipY - 2 * sc, x + 9 * sc, hipY - 2 * sc, x + 7 * sc, hipY + 9 * sc, x - 7 * sc, hipY + 9 * sc]); g.endFill();
    g.beginFill(S(cloth, 1.1)); g.drawPolygon([x - 9 * sc, hipY - 2 * sc, x - 1 * sc, hipY - 2 * sc, x - 2 * sc, hipY + 9 * sc, x - 7 * sc, hipY + 9 * sc]); g.endFill();
    // torso (V-taper to broad shoulders)
    g.beginFill(t.mid); g.drawPolygon([x - 5 * sc, hipY, x + 5 * sc, hipY, x + 11 * sc, shoY, x - 11 * sc, shoY]); g.endFill();
    g.beginFill(t.lit); g.drawPolygon([x - 5 * sc, hipY, x - 0.5 * sc, hipY, x - 1 * sc, shoY, x - 11 * sc, shoY]); g.endFill();
    g.beginFill(t.dk, 0.55); g.drawPolygon([x + 2 * sc, hipY, x + 5 * sc, hipY, x + 11 * sc, shoY, x + 5 * sc, shoY]); g.endFill();
    // pectoral / ab hints
    g.lineStyle({ width: 1, color: t.dk, alpha: 0.4 });
    g.moveTo(x - 6 * sc, shoY + 5 * sc); g.lineTo(x + 6 * sc, shoY + 5 * sc);
    g.moveTo(x, shoY + 4 * sc); g.lineTo(x, hipY - 2 * sc); g.lineStyle(0);
    // left arm down at side (viewer right)
    taper(g, x + 10 * sc, shoY + 1 * sc, 3 * sc, x + 12 * sc, hipY + 2 * sc, 2.4 * sc, t.dk);
    // right arm raised holding torch (viewer left)
    const handX = x - 17 * sc, handY = y - 96 * sc;
    taper(g, x - 10 * sc, shoY + 1 * sc, 3.2 * sc, x - 15 * sc, shoY - 16 * sc, 2.8 * sc, t.lit);
    taper(g, x - 15 * sc, shoY - 16 * sc, 2.8 * sc, handX, handY, 2.4 * sc, t.lit);
    // neck + head
    g.beginFill(t.mid); g.drawRect(x - 2.2 * sc, headY, 4.4 * sc, shoY - headY + 2 * sc); g.endFill();
    g.beginFill(t.lit); g.drawCircle(x - 0.6 * sc, headY - 4 * sc, 6 * sc); g.endFill();
    g.beginFill(t.dk, 0.5); g.drawEllipse(x + 3 * sc, headY - 4 * sc, 2.6 * sc, 6 * sc); g.endFill();
    // radiate crown (Helios) — sun rays around the head
    if (opt.helios) {
      g.lineStyle({ width: 1.6 * sc, color: t.hl, alpha: 0.95 });
      for (let i = 0; i < 9; i++) {
        const a = -Math.PI * 0.95 + (i / 8) * Math.PI * 0.9;
        const hx = x - 0.6 * sc + Math.cos(a) * 6.5 * sc, hy = headY - 4 * sc + Math.sin(a) * 6.5 * sc;
        g.moveTo(hx, hy); g.lineTo(hx + Math.cos(a) * 6 * sc, hy + Math.sin(a) * 6 * sc);
      }
      g.lineStyle(0);
    }
    // torch cup at the raised hand (flame added separately by caller)
    if (opt.torch) {
      g.beginFill(t.dk); g.drawRect(handX - 3 * sc, handY - 1 * sc, 6 * sc, 3 * sc); g.endFill();
      g.beginFill(t.hl); g.drawEllipse(handX, handY - 3 * sc, 4 * sc, 2 * sc); g.endFill();
    }
    return { torch: { x: handX, y: handY }, head: { x: x, y: headY - 4 * sc } };
  }

  // ---- enthroned deity (Zeus at Olympia) — chryselephantine -------------
  function enthroned(g, x, y, sc, opt) {
    opt = opt || {};
    const gold = opt.gold || M.gold, ivory = opt.ivory || M.marble;
    const tg = tone(gold), ti = tone(ivory);
    shadow(g, x, y, sc, 22);
    // throne block (behind/under)
    const thrTop = y - 30 * sc;
    g.beginFill(S(gold, 0.7)); g.drawRect(x - 24 * sc, thrTop, 48 * sc, 30 * sc); g.endFill();   // seat base
    g.beginFill(S(gold, 0.84)); g.drawRect(x - 24 * sc, thrTop, 6 * sc, 30 * sc); g.endFill();
    g.beginFill(S(gold, 0.6)); g.drawRect(x + 18 * sc, thrTop, 6 * sc, 30 * sc); g.endFill();
    // throne back + finials
    g.beginFill(S(gold, 0.78)); g.drawRect(x - 24 * sc, y - 78 * sc, 7 * sc, 48 * sc); g.drawRect(x + 17 * sc, y - 78 * sc, 7 * sc, 48 * sc); g.endFill();
    g.beginFill(tg.hl); g.drawCircle(x - 20.5 * sc, y - 80 * sc, 3.4 * sc); g.drawCircle(x + 20.5 * sc, y - 80 * sc, 3.4 * sc); g.endFill();
    g.beginFill(S(gold, 0.66)); g.drawRect(x - 18 * sc, y - 72 * sc, 36 * sc, 42 * sc); g.endFill(); // back panel
    g.lineStyle({ width: 1, color: tg.hl, alpha: 0.4 }); g.drawRect(x - 15 * sc, y - 68 * sc, 30 * sc, 34 * sc); g.lineStyle(0);
    // lap drapery (himation over legs) — gold
    g.beginFill(tg.mid); g.drawPolygon([x - 18 * sc, thrTop - 2 * sc, x + 18 * sc, thrTop - 2 * sc, x + 16 * sc, thrTop + 16 * sc, x - 16 * sc, thrTop + 16 * sc]); g.endFill();
    g.beginFill(tg.lit); g.drawPolygon([x - 18 * sc, thrTop - 2 * sc, x - 4 * sc, thrTop - 2 * sc, x - 5 * sc, thrTop + 16 * sc, x - 16 * sc, thrTop + 16 * sc]); g.endFill();
    g.lineStyle({ width: 1, color: tg.dk, alpha: 0.45 });
    for (let i = -3; i <= 3; i++) { g.moveTo(x + i * 5 * sc, thrTop); g.lineTo(x + i * 5 * sc + 1.5 * sc, thrTop + 15 * sc); } g.lineStyle(0);
    // lower legs / feet on footstool
    g.beginFill(ti.mid); g.drawRect(x - 11 * sc, thrTop + 14 * sc, 7 * sc, 18 * sc); g.drawRect(x + 4 * sc, thrTop + 14 * sc, 7 * sc, 18 * sc); g.endFill();
    g.beginFill(S(gold, 0.8)); g.drawRect(x - 14 * sc, y - 4 * sc, 28 * sc, 5 * sc); g.endFill(); // footstool
    // bare torso (ivory)
    const shoY = y - 60 * sc;
    g.beginFill(ti.mid); g.drawPolygon([x - 9 * sc, thrTop, x + 9 * sc, thrTop, x + 12 * sc, shoY, x - 12 * sc, shoY]); g.endFill();
    g.beginFill(ti.lit); g.drawPolygon([x - 9 * sc, thrTop, x - 1 * sc, thrTop, x - 2 * sc, shoY, x - 12 * sc, shoY]); g.endFill();
    // himation over left shoulder (gold sash)
    g.beginFill(tg.mid); g.drawPolygon([x + 4 * sc, shoY - 2 * sc, x + 12 * sc, shoY, x + 8 * sc, thrTop, x + 2 * sc, thrTop]); g.endFill();
    // arms: right extended holding a small Nike, left raised on a sceptre
    g.beginFill(ti.lit); taper(g, x - 11 * sc, shoY + 2 * sc, 3 * sc, x - 22 * sc, shoY + 4 * sc, 2.4 * sc, ti.lit); g.endFill();
    g.beginFill(ti.dk); taper(g, x + 11 * sc, shoY + 2 * sc, 3 * sc, x + 19 * sc, shoY - 4 * sc, 2.4 * sc, ti.dk); g.endFill();
    // sceptre (left hand, viewer right)
    g.beginFill(S(gold, 0.82)); g.drawRect(x + 18 * sc, y - 92 * sc, 2.2 * sc, 88 * sc); g.endFill();
    g.beginFill(tg.hl); g.drawCircle(x + 19 * sc, y - 92 * sc, 3.6 * sc); g.endFill(); // eagle finial
    // little Nike figurine on right palm (viewer left)
    miniNike(g, x - 24 * sc, shoY + 4 * sc, sc * 0.5, gold);
    // head — bearded, olive wreath
    const headY = y - 70 * sc;
    g.beginFill(ti.lit); g.drawCircle(x, headY, 6.5 * sc); g.endFill();
    g.beginFill(S(M.wood, 0.95)); g.drawEllipse(x, headY + 4 * sc, 6 * sc, 5 * sc); g.endFill(); // beard
    g.beginFill(S(M.wood, 0.8)); g.drawEllipse(x - 2 * sc, headY - 4 * sc, 7 * sc, 4 * sc); g.endFill(); // hair
    g.lineStyle({ width: 1.6 * sc, color: M.leafDk }); g.arc(x, headY - 2 * sc, 8 * sc, Math.PI * 1.05, Math.PI * 1.95); g.lineStyle(0); // wreath
    return { head: { x, y: headY } };
  }

  // ---- standing goddess (Athena Parthenos) ------------------------------
  function goddess(g, x, y, sc, opt) {
    opt = opt || {};
    const gold = opt.gold || M.gold, ivory = opt.ivory || M.marble;
    const tg = tone(gold), ti = tone(ivory);
    shadow(g, x, y, sc, 13);
    const hemY = y, kneeY = y - 30 * sc, waistY = y - 48 * sc, shoY = y - 64 * sc, headY = y - 76 * sc;
    // peplos skirt (gold, bell drape with vertical folds)
    g.beginFill(tg.mid); g.drawPolygon([x - 13 * sc, hemY, x + 13 * sc, hemY, x + 7 * sc, waistY, x - 7 * sc, waistY]); g.endFill();
    g.beginFill(tg.lit); g.drawPolygon([x - 13 * sc, hemY, x - 3 * sc, hemY, x - 3 * sc, waistY, x - 7 * sc, waistY]); g.endFill();
    g.beginFill(tg.dk, 0.5); g.drawPolygon([x + 4 * sc, hemY, x + 13 * sc, hemY, x + 7 * sc, waistY, x + 3 * sc, waistY]); g.endFill();
    g.lineStyle({ width: 1, color: tg.dk, alpha: 0.5 });
    for (let i = -3; i <= 3; i++) { g.moveTo(x + i * 3.6 * sc, waistY + 2 * sc); g.lineTo(x + i * 4.6 * sc, hemY - 1 * sc); } g.lineStyle(0);
    g.beginFill(tg.hl, 0.6); g.drawRect(x - 13 * sc, hemY - 2 * sc, 26 * sc, 2 * sc); g.endFill(); // hem band
    void kneeY;
    // upper body (peplos, gold) with aegis bib (ivory scales)
    g.beginFill(tg.mid); g.drawPolygon([x - 7 * sc, waistY, x + 7 * sc, waistY, x + 9 * sc, shoY, x - 9 * sc, shoY]); g.endFill();
    g.beginFill(tg.lit); g.drawPolygon([x - 7 * sc, waistY, x - 1 * sc, waistY, x - 2 * sc, shoY, x - 9 * sc, shoY]); g.endFill();
    g.beginFill(ti.lit); g.drawPolygon([x - 6 * sc, shoY + 1 * sc, x + 6 * sc, shoY + 1 * sc, x, shoY + 9 * sc]); g.endFill(); // aegis
    // arms: right extended forward holding Nike, left hand resting (shield)
    g.beginFill(ti.lit); taper(g, x - 8 * sc, shoY + 2 * sc, 2.6 * sc, x - 20 * sc, shoY + 8 * sc, 2 * sc, ti.lit); g.endFill();
    g.beginFill(ti.dk); taper(g, x + 8 * sc, shoY + 2 * sc, 2.6 * sc, x + 12 * sc, waistY, 2 * sc, ti.dk); g.endFill();
    // Nike on the outstretched right palm
    miniNike(g, x - 22 * sc, shoY + 8 * sc, sc * 0.46, gold);
    // big round shield resting at left side (viewer right), + coiled spear
    g.beginFill(S(gold, 0.7)); g.drawCircle(x + 17 * sc, y - 18 * sc, 14 * sc); g.endFill();
    g.beginFill(tg.lit); g.drawCircle(x + 14 * sc, y - 21 * sc, 12 * sc); g.endFill();
    g.beginFill(S(gold, 0.62)); g.drawCircle(x + 17 * sc, y - 18 * sc, 4.5 * sc); g.endFill(); // boss
    g.lineStyle({ width: 1, color: S(gold, 0.6), alpha: 0.5 }); g.drawCircle(x + 16 * sc, y - 19 * sc, 8.5 * sc); g.lineStyle(0);
    g.beginFill(S(M.bronze, 0.9)); g.drawRect(x + 22 * sc, y - 92 * sc, 1.8 * sc, 92 * sc); g.endFill(); // spear
    // neck + head with high-crested Attic helmet
    g.beginFill(ti.mid); g.drawRect(x - 2.4 * sc, headY, 4.8 * sc, shoY - headY + 2 * sc); g.endFill();
    g.beginFill(ti.lit); g.drawCircle(x, headY - 4 * sc, 5.6 * sc); g.endFill();
    g.beginFill(tg.mid); g.drawRect(x - 6 * sc, headY - 12 * sc, 12 * sc, 6 * sc); g.endFill(); // helmet bowl
    g.beginFill(tg.dk); g.drawPolygon([x - 6 * sc, headY - 11 * sc, x + 6 * sc, headY - 11 * sc, x + 7 * sc, headY - 7 * sc, x - 7 * sc, headY - 7 * sc]); g.endFill();
    // crest
    g.beginFill(S(M.red, 1.0)); g.drawPolygon([x - 5 * sc, headY - 12 * sc, x + 5 * sc, headY - 12 * sc, x + 7 * sc, headY - 22 * sc, x - 3 * sc, headY - 19 * sc]); g.endFill();
    g.beginFill(S(M.red, 0.7)); g.drawPolygon([x - 5 * sc, headY - 12 * sc, x - 1 * sc, headY - 12 * sc, x + 1 * sc, headY - 19 * sc, x - 3 * sc, headY - 18 * sc]); g.endFill();
    return { head: { x, y: headY } };
  }

  // ---- a small winged Victory (held in a deity's hand, or standalone) ---
  function miniNike(g, x, y, sc, mat) {
    mat = mat || M.gold; const t = tone(mat);
    g.beginFill(t.mid); g.drawPolygon([x - 3 * sc, y, x + 3 * sc, y, x + 2 * sc, y - 10 * sc, x - 2 * sc, y - 10 * sc]); g.endFill(); // body
    g.beginFill(t.lit); g.drawCircle(x, y - 12 * sc, 2.4 * sc); g.endFill(); // head
    // wings
    g.beginFill(t.lit, 0.95); g.drawPolygon([x - 2 * sc, y - 9 * sc, x - 12 * sc, y - 16 * sc, x - 9 * sc, y - 5 * sc]); g.endFill();
    g.beginFill(t.dk, 0.9); g.drawPolygon([x + 2 * sc, y - 9 * sc, x + 12 * sc, y - 16 * sc, x + 9 * sc, y - 5 * sc]); g.endFill();
  }

  // ---- caryatid maiden (Erechtheion porch) supporting an entablature ----
  function caryatid(g, x, y, sc, mat) {
    mat = mat || M.marble; const t = tone(mat);
    shadow(g, x, y, sc, 9);
    const hemY = y, waistY = y - 30 * sc, shoY = y - 44 * sc, headY = y - 52 * sc;
    // column-like fluted skirt
    g.beginFill(t.mid); g.drawPolygon([x - 7 * sc, hemY, x + 7 * sc, hemY, x + 5 * sc, waistY, x - 5 * sc, waistY]); g.endFill();
    g.beginFill(t.lit); g.drawPolygon([x - 7 * sc, hemY, x - 2 * sc, hemY, x - 2 * sc, waistY, x - 5 * sc, waistY]); g.endFill();
    g.lineStyle({ width: 1, color: t.dk, alpha: 0.5 });
    for (let i = -2; i <= 2; i++) { g.moveTo(x + i * 2.6 * sc, waistY + 1 * sc); g.lineTo(x + i * 3.2 * sc, hemY - 1 * sc); } g.lineStyle(0);
    // torso (peplos)
    g.beginFill(t.mid); g.drawPolygon([x - 5 * sc, waistY, x + 5 * sc, waistY, x + 6 * sc, shoY, x - 6 * sc, shoY]); g.endFill();
    g.beginFill(t.lit); g.drawPolygon([x - 5 * sc, waistY, x - 1 * sc, waistY, x - 2 * sc, shoY, x - 6 * sc, shoY]); g.endFill();
    g.beginFill(t.dk, 0.5); g.drawRect(x + 3 * sc, shoY, 3 * sc, waistY - shoY); g.endFill();
    // arm hint at side
    g.beginFill(t.dk); g.drawRect(x + 5 * sc, shoY + 1 * sc, 2.2 * sc, 14 * sc); g.endFill();
    // neck + head + capital block above head
    g.beginFill(t.mid); g.drawRect(x - 2 * sc, headY, 4 * sc, shoY - headY + 1 * sc); g.endFill();
    g.beginFill(t.lit); g.drawCircle(x - 0.5 * sc, headY - 2 * sc, 4.2 * sc); g.endFill();
    g.beginFill(t.dk, 0.4); g.drawEllipse(x + 2.4 * sc, headY - 2 * sc, 1.8 * sc, 4 * sc); g.endFill();
    // hair coils on shoulders
    g.beginFill(t.dk, 0.5); g.drawRect(x - 5 * sc, headY, 2 * sc, 8 * sc); g.drawRect(x + 3 * sc, headY, 2 * sc, 8 * sc); g.endFill();
    // capital (echinus + abacus) she carries
    g.beginFill(t.lit); g.drawRect(x - 6 * sc, headY - 10 * sc, 12 * sc, 4 * sc); g.endFill();
    g.beginFill(t.mid); g.drawRect(x - 4.5 * sc, headY - 6 * sc, 9 * sc, 2 * sc); g.endFill();
  }

  // ---- quadriga (4-horse chariot) atop a monument -----------------------
  function quadriga(g, x, y, sc, mat) {
    mat = mat || M.bronze; const t = tone(mat);
    shadow(g, x, y, sc, 22);
    // 4 horses abreast (overlapping silhouettes), facing viewer-left
    for (let i = 3; i >= 0; i--) {
      const hx = x - 6 * sc + i * 6 * sc, lit = i < 2 ? t.dk : t.lit;
      g.beginFill(lit); g.drawEllipse(hx, y - 12 * sc, 9 * sc, 6 * sc); g.endFill(); // body
      g.beginFill(lit); g.drawPolygon([hx - 9 * sc, y - 14 * sc, hx - 16 * sc, y - 22 * sc, hx - 13 * sc, y - 23 * sc, hx - 6 * sc, y - 13 * sc]); g.endFill(); // neck/head
      g.beginFill(S(mat, i < 2 ? 0.5 : 0.85)); g.drawRect(hx - 2 * sc, y - 8 * sc, 2 * sc, 8 * sc); g.drawRect(hx + 3 * sc, y - 8 * sc, 2 * sc, 8 * sc); g.endFill(); // legs
    }
    // chariot car + driver behind
    g.beginFill(S(mat, 0.7)); g.drawRect(x + 10 * sc, y - 16 * sc, 12 * sc, 12 * sc); g.endFill();
    g.beginFill(t.lit); g.drawCircle(x + 12 * sc, y - 6 * sc, 5 * sc); g.endFill(); // wheel
    g.lineStyle({ width: 1, color: t.dk2, alpha: 0.6 }); for (let k = 0; k < 4; k++) { const a = k * Math.PI / 4; g.moveTo(x + 12 * sc, y - 6 * sc); g.lineTo(x + 12 * sc + Math.cos(a) * 5 * sc, y - 6 * sc + Math.sin(a) * 5 * sc); } g.lineStyle(0);
    g.beginFill(t.mid); g.drawRect(x + 14 * sc, y - 30 * sc, 5 * sc, 16 * sc); g.endFill(); // driver torso
    g.beginFill(t.lit); g.drawCircle(x + 16.5 * sc, y - 32 * sc, 3 * sc); g.endFill(); // head
  }

  // ---- winged Victory on a ship prow (Nike of Samothrace) ---------------
  function wingedVictory(g, x, y, sc, opt) {
    opt = opt || {}; const mat = opt.mat || M.marble; const t = tone(mat);
    shadow(g, x, y, sc, 16);
    // ship prow base (angled blocks)
    g.beginFill(S(mat, 0.78)); g.drawPolygon([x - 18 * sc, y, x + 16 * sc, y - 4 * sc, x + 22 * sc, y - 12 * sc, x - 14 * sc, y - 8 * sc]); g.endFill();
    g.beginFill(S(mat, 0.62)); g.drawPolygon([x - 18 * sc, y, x - 14 * sc, y - 8 * sc, x + 22 * sc, y - 12 * sc, x + 22 * sc, y - 6 * sc]); g.endFill();
    g.lineStyle({ width: 1, color: t.dk, alpha: 0.4 }); for (let i = 0; i < 4; i++) { const u = i / 4; g.moveTo(x - 14 * sc + u * 34 * sc, y - 8 * sc); g.lineTo(x - 18 * sc + u * 36 * sc, y); } g.lineStyle(0);
    const baseY = y - 12 * sc;
    // wind-blown drapery skirt
    g.beginFill(t.mid); g.drawPolygon([x - 10 * sc, baseY, x + 10 * sc, baseY - 2 * sc, x + 6 * sc, baseY - 30 * sc, x - 5 * sc, baseY - 30 * sc]); g.endFill();
    g.beginFill(t.lit); g.drawPolygon([x - 10 * sc, baseY, x - 2 * sc, baseY, x - 2 * sc, baseY - 30 * sc, x - 5 * sc, baseY - 30 * sc]); g.endFill();
    g.lineStyle({ width: 1, color: t.dk, alpha: 0.5 }); for (let i = -3; i <= 3; i++) { g.moveTo(x + i * 2.6 * sc, baseY - 28 * sc); g.lineTo(x + i * 3.4 * sc + 3 * sc, baseY - 1 * sc); } g.lineStyle(0);
    // torso
    const shoY = baseY - 44 * sc;
    g.beginFill(t.mid); g.drawPolygon([x - 5 * sc, baseY - 30 * sc, x + 5 * sc, baseY - 30 * sc, x + 8 * sc, shoY, x - 7 * sc, shoY]); g.endFill();
    g.beginFill(t.lit); g.drawPolygon([x - 5 * sc, baseY - 30 * sc, x - 1 * sc, baseY - 30 * sc, x - 2 * sc, shoY, x - 7 * sc, shoY]); g.endFill();
    // two great wings sweeping up-back
    function wing(dx, lit) {
      g.beginFill(lit, 0.96);
      g.drawPolygon([x + dx * 5 * sc, shoY + 2 * sc, x + dx * 30 * sc, shoY - 30 * sc, x + dx * 26 * sc, shoY - 10 * sc, x + dx * 18 * sc, shoY - 2 * sc]);
      g.endFill();
      g.lineStyle({ width: 1, color: t.dk, alpha: 0.45 });
      for (let i = 1; i < 5; i++) { const u = i / 5; g.moveTo(x + dx * (5 + u * 13) * sc, shoY + 2 * sc - u * 2 * sc); g.lineTo(x + dx * (5 + u * 25) * sc, shoY - u * 30 * sc); }
      g.lineStyle(0);
    }
    wing(1, t.dk); wing(-1, t.lit);
    // head (headless icon, but give a hint of neck) — keep subtle nub
    g.beginFill(t.mid); g.drawRect(x - 2 * sc, shoY - 2 * sc, 4 * sc, 4 * sc); g.endFill();
  }

  global.FIG = { heroicMale, enthroned, goddess, caryatid, quadriga, wingedVictory, miniNike };
})(window);
