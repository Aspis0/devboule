/* =========================================================================
   app.js — Showcase + interactions for the Polis building atlas (PixiJS v7)
   ========================================================================= */
(function () {
  'use strict';

  const TIERS = ['Kalybe', 'Oikia', 'Synoikia', 'Megaron', 'Mnemeion'];
  const META = window.BUILD_META;
  const ORDER = META.order;
  const PAVED = new Set(['temple', 'market', 'baths', 'library', 'townhall']);

  const state = { level: 3, anim: true, grid: false, sun: 'NW', outline: false, view: 'gallery', selType: 'temple' };

  const canvas = document.getElementById('stage');
  const wrap = document.getElementById('stagewrap');

  const app = new PIXI.Application({
    view: canvas, antialias: true, resolution: Math.min(2, window.devicePixelRatio || 1),
    autoDensity: true, backgroundColor: 0xC9D4D4, preserveDrawingBuffer: true
  });
  // soft sky gradient via a background graphics
  const bg = new PIXI.Graphics();
  app.stage.addChildAt(bg, 0);
  function paintBg(w, h) {
    bg.clear();
    bg.beginFill(0xD7DEDA); bg.drawRect(0, 0, w, h); bg.endFill();
    bg.beginFill(0xE8E2D2, 0.6); bg.drawRect(0, 0, w, h * 0.5); bg.endFill();
  }

  const world = new PIXI.Container();
  app.stage.addChild(world);
  let activeAnims = [];
  let T = 0;

  // ---- sizing ------------------------------------------------------------
  function fit() {
    const w = wrap.clientWidth, h = wrap.clientHeight;
    app.renderer.resize(w, h);
    paintBg(w, h);
  }
  window.addEventListener('resize', () => { if (window.__lockView) return; fit(); centerView(); });

  // ---- contact shadow ----------------------------------------------------
  function shadow(g, proj, W, D) {
    const c = proj.p(W / 2, D / 2, 0);
    g.beginFill(0x2a2a25, 0.13);
    g.drawEllipse(c.x, c.y, (W + D) * ISO.HALF_W * 0.42, (W + D) * ISO.HALF_H * 0.42);
    g.endFill();
  }

  // ======================================================================
  //  GALLERY
  // ======================================================================
  let galleryC = null;
  function buildGallery() {
    if (galleryC) { galleryC.destroy({ children: true }); }
    activeAnims = [];
    galleryC = new PIXI.Container();
    world.addChild(galleryC);

    const COLS = 5, CW = 300, CH = 296, TARGET_W = 244, TARGET_H = 210;
    ISO.SUN.dir = state.sun;
    const Lidx = state.level - 1;

    ORDER.forEach((type, i) => {
      const col = i % COLS, rowi = (i / COLS) | 0;
      const cellX = col * CW, cellY = rowi * CH;
      const info = META.info[type];

      // build the model
      const res = window.BUILD[type](Lidx, { outline: state.outline });
      const W = res.foot[0], D = res.foot[1];

      const wrapper = new PIXI.Container();
      const plot = new PIXI.Graphics();
      wrapper.addChild(plot);
      const proj = ISO.makeProj(W, D);
      const paved = PAVED.has(type);
      if (type !== 'harbor') {
        ISO.ground(plot, proj, W, D, paved ? { paved: true, color: ISO.MAT.stone } : {});
        shadow(plot, proj, W, D);
        if (state.grid) ISO.ground(plot, proj, W, D, { grid: true, edge: true, paved: paved, color: paved ? ISO.MAT.stone : ISO.MAT.ground });
      } else {
        shadow(plot, proj, W, D);
      }
      wrapper.addChild(res.container);
      res.anims.forEach(a => activeAnims.push(a));

      // scale to fit cell
      const b = wrapper.getLocalBounds();
      const sc = Math.min(TARGET_W / b.width, TARGET_H / b.height, 1.35);
      wrapper.scale.set(sc);
      wrapper.position.set(
        cellX + CW / 2 - (b.x + b.width / 2) * sc,
        cellY + CH / 2 - 18 - (b.y + b.height / 2) * sc
      );
      galleryC.addChild(wrapper);

      // labels
      const name = new PIXI.Text(info.name, { fontFamily: 'Cinzel, serif', fontSize: 16, fontWeight: '600', fill: 0x2c2722 });
      name.anchor.set(0.5, 0); name.position.set(cellX + CW / 2, cellY + CH - 50);
      galleryC.addChild(name);
      const meta = new PIXI.Text(type + '  ·  ' + W + '×' + D + '  ·  L' + state.level,
        { fontFamily: 'Spline Sans Mono, monospace', fontSize: 10.5, fill: 0x8a8170 });
      meta.anchor.set(0.5, 0); meta.position.set(cellX + CW / 2, cellY + CH - 30);
      galleryC.addChild(meta);
      // accent ticks
      const dot = new PIXI.Graphics();
      dot.beginFill(info.accent); dot.drawCircle(cellX + CW / 2 - name.width / 2 - 10, cellY + CH - 42, 3.2); dot.endFill();
      galleryC.addChild(dot);
    });
    updateHud();
  }

  // ======================================================================
  //  MAP (place buildings on an iso grid)
  // ======================================================================
  let mapC = null, mapProj = null, placed = [];
  let peopleC = null;
  const MAPN = 9;
  function buildMap(initial) {
    if (mapC) { mapC.destroy({ children: true }); }
    activeAnims = [];
    mapC = new PIXI.Container();
    world.addChild(mapC);
    ISO.SUN.dir = state.sun;
    mapProj = ISO.makeProj(MAPN, MAPN);

    // ground
    const gnd = new PIXI.Graphics();
    ISO.ground(gnd, mapProj, MAPN, MAPN, { grid: true, edge: true, color: 0xCDC2A2 });
    mapC.addChild(gnd);

    if (initial && placed.length === 0) {
      placed = [
        { type: 'temple', x: 1, y: 1 }, { type: 'house', x: 5, y: 1 },
        { type: 'lighthouse', x: 6, y: 5 }, { type: 'market', x: 1, y: 5 },
        { type: 'house', x: 4, y: 4 }, { type: 'tower', x: 7, y: 1 }
      ];
    }
    // depth sort & render
    const Lidx = state.level - 1;
    placed.slice().sort((a, b) => {
      const fa = window.BUILD[a.type](0, {}).foot, fb = window.BUILD[b.type](0, {}).foot;
      return (a.x + fa[0] + a.y + fa[1]) - (b.x + fb[0] + b.y + fb[1]);
    }).forEach(pl => {
      const res = window.BUILD[pl.type](Lidx, { outline: state.outline });
      const W = res.foot[0], D = res.foot[1];
      // clamp footprint into the grid
      const ox = Math.min(pl.x, MAPN - W), oy = Math.min(pl.y, MAPN - D);
      const at = mapProj.p(ox + W, oy + D, 0);
      res.container.position.set(at.x, at.y);
      mapC.addChild(res.container);
      res.anims.forEach(a => activeAnims.push(a));
    });

    // --- citizens walking the streets (aesthetic only) ---
    const sp = (gx, gy) => mapProj.p(gx, gy, 0.04);
    const loop = [sp(1, 8), sp(8, 8), sp(8, 8.6), sp(1, 8.6)];
    const loop2 = [sp(0.6, 7.6), sp(0.6, 0.8), sp(1.15, 0.8), sp(1.15, 7.6)];
    const folk = [
      PEOPLE.make('citizen', { scale: 1, path: loop, speed: 22 }),
      PEOPLE.make('merchant', { scale: 1, path: loop, speed: 16 }),
      PEOPLE.make('watercarrier', { scale: 1, path: loop2, speed: 18 }),
      PEOPLE.make('noble', { scale: 1, path: loop2, speed: 12 }),
      PEOPLE.make('citizen', { scale: 1, path: loop, speed: 27 }),
      PEOPLE.make('firefighter', { scale: 1, path: loop, speed: 21 })
    ];
    folk.forEach((p, i) => { p.seg = i % p.path.length; p.u = (i * 0.31) % 1; });
    const builder = PEOPLE.make('builder', { scale: 1, face: -1 });
    const bp = sp(4.2, 7.25); builder.node.position.set(bp.x, bp.y);
    folk.push(builder);
    folk.forEach(p => { mapC.addChild(p.node); activeAnims.push(p); });

    updateHud();
  }

  function screenToCell(globalX, globalY) {
    const lp = mapC.toLocal(new PIXI.Point(globalX, globalY));
    const N = MAPN, HW = ISO.HALF_W, HH = ISO.HALF_H, ay = 2 * N * HH;
    const sum = (lp.y + ay) / HH;     // gx+gy
    const dif = lp.x / HW;            // gx-gy
    const gx = Math.floor((sum + dif) / 2);
    const gy = Math.floor((sum - dif) / 2);
    return { gx, gy };
  }

  // ======================================================================
  //  PEOPLE showcase (citizen types)
  // ======================================================================
  function plotDisc(g, cx, cy, r) {
    g.beginFill(ISO.MAT.grassDk); g.drawEllipse(cx, cy, r, r * 0.42); g.endFill();
    g.beginFill(ISO.MAT.grass, 0.55); g.drawEllipse(cx, cy - 1, r * 0.92, r * 0.36); g.endFill();
    g.beginFill(0x241a10, 0.1); g.drawEllipse(cx + 2, cy + 1, r * 0.7, r * 0.26); g.endFill();
  }
  function buildPeople() {
    if (peopleC) peopleC.destroy({ children: true });
    activeAnims = [];
    peopleC = new PIXI.Container();
    world.addChild(peopleC);
    const COLS = 3, CW = 300, CH = 286, SC = 3;
    PEOPLE.order.forEach((type, i) => {
      const col = i % COLS, row = (i / COLS) | 0;
      const cx = col * CW + CW / 2, cy = row * CH + CH / 2 + 40;
      const plot = new PIXI.Graphics(); peopleC.addChild(plot);
      plotDisc(plot, cx, cy, 78);
      // contextual extras
      if (type === 'builder') {
        // a half-built marble wall to hammer
        const bx = cx + 42, by = cy + 6;
        plot.beginFill(ISO.shade(ISO.MAT.marble, 0.7)); plot.drawPolygon([bx, by, bx + 30, by - 16, bx + 30, by - 58, bx, by - 42]); plot.endFill();
        plot.beginFill(ISO.shade(ISO.MAT.marble, 1.05)); plot.drawPolygon([bx, by - 42, bx + 30, by - 58, bx + 30, by - 70, bx, by - 54]); plot.endFill();
        plot.lineStyle({ width: 1, color: ISO.shade(ISO.MAT.marble, 0.55), alpha: 0.5 });
        for (let k = 1; k < 4; k++) { plot.moveTo(bx, by - 14 * k); plot.lineTo(bx + 30, by - 16 - 14 * k); } plot.lineStyle(0);
      }
      let p;
      if (type === 'builder') { p = PEOPLE.make(type, { scale: SC, face: 1 }); p.node.position.set(cx - 14, cy); }
      else if (type === 'firefighter') {
        p = PEOPLE.make(type, { scale: SC, face: 1 }); p.node.position.set(cx - 26, cy);
        const fp = { x: cx + 40, y: cy - 2 };
        const fl = new ANIM.Flame(fp.x, fp.y, 2.1); peopleC.addChild(fl.node); activeAnims.push(fl);
      } else {
        p = PEOPLE.make(type, { scale: SC, range: { x0: cx - 52, x1: cx + 52, y: cy }, x: cx });
      }
      peopleC.addChild(p.node); activeAnims.push(p);
      // labels
      const name = new PIXI.Text(PEOPLE.info[type].name, { fontFamily: 'Cinzel, serif', fontSize: 17, fontWeight: '600', fill: 0x2c2722 });
      name.anchor.set(0.5, 0); name.position.set(cx, row * CH + CH - 52); peopleC.addChild(name);
      const meta = new PIXI.Text(type + '  ·  ' + PEOPLE.info[type].it, { fontFamily: 'Spline Sans Mono, monospace', fontSize: 10.5, fill: 0x8a8170 });
      meta.anchor.set(0.5, 0); meta.position.set(cx, row * CH + CH - 30); peopleC.addChild(meta);
    });
    updateHud();
  }

  // ======================================================================
  //  view switching + centering
  // ======================================================================
  function rebuild() {
    if (galleryC) galleryC.visible = state.view === 'gallery';
    if (mapC) mapC.visible = state.view === 'map';
    if (peopleC) peopleC.visible = state.view === 'people';
    if (state.view === 'gallery') buildGallery();
    else if (state.view === 'map') buildMap(true);
    else buildPeople();
    centerView();
  }
  function centerView() {
    const cont = state.view === 'gallery' ? galleryC : state.view === 'map' ? mapC : peopleC;
    if (!cont) return;
    const vw = wrap.clientWidth, vh = wrap.clientHeight;
    const b = cont.getLocalBounds();
    const m = 48;
    const sc = Math.min((vw - m) / b.width, (vh - m) / b.height, 1.1);
    world.scale.set(sc);
    world.position.set(vw / 2 - (b.x + b.width / 2) * sc, vh / 2 - (b.y + b.height / 2) * sc);
    updateHud();
  }

  // ======================================================================
  //  pan / zoom
  // ======================================================================
  let dragging = false, last = null;
  canvas.addEventListener('pointerdown', (e) => {
    if (state.view === 'map' && e.button === 0) {
      const r = canvas.getBoundingClientRect();
      const cell = screenToCell(e.clientX - r.left, e.clientY - r.top);
      if (cell.gx >= 0 && cell.gy >= 0 && cell.gx < MAPN && cell.gy < MAPN) {
        placed.push({ type: state.selType, x: cell.gx, y: cell.gy });
        buildMap(false); return;
      }
    }
    dragging = true; last = { x: e.clientX, y: e.clientY };
  });
  window.addEventListener('pointermove', (e) => {
    if (!dragging) return;
    world.position.x += e.clientX - last.x;
    world.position.y += e.clientY - last.y;
    last = { x: e.clientX, y: e.clientY };
  });
  window.addEventListener('pointerup', () => { dragging = false; });
  canvas.addEventListener('contextmenu', (e) => {
    e.preventDefault();
    if (state.view !== 'map') return;
    const r = canvas.getBoundingClientRect();
    const cell = screenToCell(e.clientX - r.left, e.clientY - r.top);
    // remove topmost occupying that cell
    for (let i = placed.length - 1; i >= 0; i--) {
      const pl = placed[i]; const foot = window.BUILD[pl.type](0, {}).foot;
      if (cell.gx >= pl.x && cell.gx < pl.x + foot[0] && cell.gy >= pl.y && cell.gy < pl.y + foot[1]) {
        placed.splice(i, 1); buildMap(false); break;
      }
    }
  });
  canvas.addEventListener('wheel', (e) => {
    e.preventDefault();
    const r = canvas.getBoundingClientRect();
    const mx = e.clientX - r.left, my = e.clientY - r.top;
    const factor = e.deltaY < 0 ? 1.12 : 1 / 1.12;
    const pre = { x: (mx - world.x) / world.scale.x, y: (my - world.y) / world.scale.y };
    const ns = Math.max(0.25, Math.min(3, world.scale.x * factor));
    world.scale.set(ns);
    world.position.set(mx - pre.x * ns, my - pre.y * ns);
    updateHud();
  }, { passive: false });

  // ======================================================================
  //  ticker
  // ======================================================================
  app.ticker.add(() => {
    const dt = state.anim ? Math.min(0.05, app.ticker.deltaMS / 1000) : 0;
    T += dt;
    for (const a of activeAnims) { if (a && a.update) a.update(T, dt); }
  });

  // ======================================================================
  //  HUD
  // ======================================================================
  function updateHud() {
    const hud = document.getElementById('hud');
    const z = Math.round(world.scale.x * 100);
    if (state.view === 'gallery')
      hud.innerHTML = '<b>Galleria</b> · 15 modelli · taglia <b>' + TIERS[state.level - 1] + '</b> · zoom ' + z + '%';
    else if (state.view === 'people')
      hud.innerHTML = '<b>Cittadini</b> · 6 figure animate · solo estetica · zoom ' + z + '%';
    else
      hud.innerHTML = '<b>Mappa</b> ' + MAPN + '×' + MAPN + ' · ' + placed.length + ' edifici · sole ' + state.sun + ' · zoom ' + z + '%';
  }

  // ======================================================================
  //  controls
  // ======================================================================
  function bindToggle(id, key, cb) {
    const el = document.getElementById(id);
    el.addEventListener('click', () => {
      state[key] = !state[key]; el.classList.toggle('on', state[key]); (cb || rebuild)();
    });
  }
  let _inited = false;
  function init() {
    if (_inited) return; _inited = true;
    // view seg
    document.getElementById('viewSeg').addEventListener('click', e => {
      const btn = e.target.closest('button'); if (!btn) return;
      [...e.currentTarget.children].forEach(b => b.classList.toggle('on', b === btn));
      state.view = btn.dataset.v;
      document.getElementById('mapctl').style.display = state.view === 'map' ? 'block' : 'none';
      rebuild();
    });
    // sun seg
    document.getElementById('sunSeg').addEventListener('click', e => {
      const btn = e.target.closest('button'); if (!btn) return;
      [...e.currentTarget.children].forEach(b => b.classList.toggle('on', b === btn));
      state.sun = btn.dataset.s; rebuild();
    });
    // level
    const lv = document.getElementById('level');
    lv.addEventListener('input', () => {
      state.level = +lv.value;
      document.getElementById('tierName').textContent = TIERS[state.level - 1];
      document.getElementById('tierNum').textContent = state.level + ' / 5';
      rebuild();
    });
    bindToggle('swAnim', 'anim', () => updateHud());
    bindToggle('swGrid', 'grid');
    bindToggle('swOutline', 'outline');
    // type select
    const sel = document.getElementById('typeSel');
    ORDER.forEach(t => { const o = document.createElement('option'); o.value = t; o.textContent = META.info[t].name + ' (' + t + ')'; sel.appendChild(o); });
    sel.addEventListener('change', () => { state.selType = sel.value; });
    document.getElementById('clearMap').addEventListener('click', () => { placed = []; buildMap(false); });

    fit();
    rebuild();
    window.__polis = {
      get gallery() { return galleryC; }, get map() { return mapC; },
      get world() { return world; }, state, rebuild, centerView
    };
  }

  if (document.fonts && document.fonts.ready) {
    document.fonts.ready.then(init);
    setTimeout(() => { if (!galleryC) init(); }, 1200); // fallback if fonts hang
  } else { init(); }
})();
