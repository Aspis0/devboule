/* =========================================================================
   monuments_app.js — Gallery showcase for the Monuments of Ancient Greece
   Reuses iso/anims/detail/figures + monuments_a/_b. Pan/zoom gallery.
   ========================================================================= */
(function () {
  'use strict';
  const META = window.MON_META;
  const ORDER = META.order;
  const PAVED = new Set(['parthenon', 'erechtheion', 'artemision', 'propylaia',
    'bomos', 'olympieion', 'horologion', 'zeus', 'athena']);
  const NO_GROUND = new Set(['kolossos']);

  const state = { anim: true, grid: false, sun: 'NW', outline: false, only7: false };

  const canvas = document.getElementById('stage');
  const wrap = document.getElementById('stagewrap');
  const SEVEN = new Set(['artemision', 'mausoleion', 'kolossos', 'zeus']); // the Greek Wonders

  const app = new PIXI.Application({
    view: canvas, antialias: true, resolution: Math.min(2, window.devicePixelRatio || 1),
    autoDensity: true, backgroundColor: 0xC9D4D4, preserveDrawingBuffer: true
  });
  const bg = new PIXI.Graphics();
  app.stage.addChildAt(bg, 0);
  function paintBg(w, h) {
    bg.clear();
    bg.beginFill(0xD9DFDA); bg.drawRect(0, 0, w, h); bg.endFill();
    bg.beginFill(0xEAE3D2, 0.6); bg.drawRect(0, 0, w, h * 0.52); bg.endFill();
  }

  const world = new PIXI.Container();
  app.stage.addChild(world);
  let activeAnims = [], T = 0;

  function fit() { const w = wrap.clientWidth, h = wrap.clientHeight; app.renderer.resize(w, h); paintBg(w, h); }
  window.addEventListener('resize', () => { fit(); centerView(); });

  function shadow(g, proj, W, D) {
    const c = proj.p(W / 2, D / 2, 0);
    g.beginFill(0x2a2a25, 0.13);
    g.drawEllipse(c.x, c.y, (W + D) * ISO.HALF_W * 0.42, (W + D) * ISO.HALF_H * 0.42);
    g.endFill();
  }

  let galleryC = null;
  function buildGallery() {
    if (galleryC) galleryC.destroy({ children: true });
    activeAnims = [];
    galleryC = new PIXI.Container();
    world.addChild(galleryC);
    ISO.SUN.dir = state.sun;

    const list = ORDER.filter(t => !state.only7 || SEVEN.has(t));
    const COLS = state.only7 ? 2 : 4, CW = 326, CH = 372, TARGET_W = 276, TARGET_H = 286;

    list.forEach((type, i) => {
      const col = i % COLS, rowi = (i / COLS) | 0;
      const cellX = col * CW, cellY = rowi * CH;
      const info = META.info[type];

      const res = window.MON[type]({ outline: state.outline });
      const W = res.foot[0], D = res.foot[1];

      const wrapper = new PIXI.Container();
      const plot = new PIXI.Graphics();
      wrapper.addChild(plot);
      const proj = ISO.makeProj(W, D);
      const paved = PAVED.has(type);
      if (!NO_GROUND.has(type)) {
        ISO.ground(plot, proj, W, D, paved ? { paved: true, color: ISO.MAT.stone } : {});
        shadow(plot, proj, W, D);
        if (state.grid) ISO.ground(plot, proj, W, D, { grid: true, edge: true, paved: paved, color: paved ? ISO.MAT.stone : ISO.MAT.ground });
      } else { shadow(plot, proj, W, D); }
      wrapper.addChild(res.container);
      res.anims.forEach(a => activeAnims.push(a));

      const b = wrapper.getLocalBounds();
      const sc = Math.min(TARGET_W / b.width, TARGET_H / b.height, 1.5);
      wrapper.scale.set(sc);
      wrapper.position.set(
        cellX + CW / 2 - (b.x + b.width / 2) * sc,
        cellY + CH / 2 - 30 - (b.y + b.height / 2) * sc
      );
      galleryC.addChild(wrapper);

      // name
      const name = new PIXI.Text(info.name, { fontFamily: 'Cinzel, serif', fontSize: 18, fontWeight: '700', fill: 0x2c2722 });
      name.anchor.set(0.5, 0); name.position.set(cellX + CW / 2, cellY + CH - 70);
      galleryC.addChild(name);
      // caption
      const sub = new PIXI.Text(info.sub, { fontFamily: 'Spline Sans Mono, monospace', fontSize: 11, fill: 0x6c6453 });
      sub.anchor.set(0.5, 0); sub.position.set(cellX + CW / 2, cellY + CH - 46);
      galleryC.addChild(sub);
      // meta
      const meta = new PIXI.Text(info.cat + '  ·  ' + W + '×' + D,
        { fontFamily: 'Spline Sans Mono, monospace', fontSize: 9.5, fill: 0x9a9180 });
      meta.anchor.set(0.5, 0); meta.position.set(cellX + CW / 2, cellY + CH - 28);
      galleryC.addChild(meta);
      // accent dot
      const dot = new PIXI.Graphics();
      dot.beginFill(info.accent); dot.drawCircle(cellX + CW / 2 - name.width / 2 - 11, cellY + CH - 61, 3.4); dot.endFill();
      galleryC.addChild(dot);
    });
    updateHud();
  }

  function centerView() {
    if (!galleryC) return;
    const vw = wrap.clientWidth, vh = wrap.clientHeight, b = galleryC.getLocalBounds(), m = 56;
    const sc = Math.min((vw - m) / b.width, (vh - m) / b.height, 1.1);
    world.scale.set(sc);
    world.position.set(vw / 2 - (b.x + b.width / 2) * sc, vh / 2 - (b.y + b.height / 2) * sc);
    updateHud();
  }

  // pan / zoom
  let dragging = false, last = null;
  canvas.addEventListener('pointerdown', (e) => { dragging = true; last = { x: e.clientX, y: e.clientY }; });
  window.addEventListener('pointermove', (e) => {
    if (!dragging) return;
    world.position.x += e.clientX - last.x; world.position.y += e.clientY - last.y;
    last = { x: e.clientX, y: e.clientY };
  });
  window.addEventListener('pointerup', () => { dragging = false; });
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

  app.ticker.add(() => {
    const dt = state.anim ? Math.min(0.05, app.ticker.deltaMS / 1000) : 0;
    T += dt;
    for (const a of activeAnims) { if (a && a.update) a.update(T, dt); }
  });

  function updateHud() {
    const hud = document.getElementById('hud');
    const z = Math.round(world.scale.x * 100);
    const n = state.only7 ? 4 : ORDER.length;
    hud.innerHTML = '<b>Meraviglie</b> · ' + n + ' modelli · sole ' + state.sun + ' · zoom ' + z + '%';
  }

  function bindToggle(id, key, cb) {
    const el = document.getElementById(id);
    el.addEventListener('click', () => { state[key] = !state[key]; el.classList.toggle('on', state[key]); (cb || rebuild)(); });
  }
  function rebuild() { buildGallery(); centerView(); }

  let _inited = false;
  function init() {
    if (_inited) return; _inited = true;
    document.getElementById('sunSeg').addEventListener('click', e => {
      const btn = e.target.closest('button'); if (!btn) return;
      [...e.currentTarget.children].forEach(b => b.classList.toggle('on', b === btn));
      state.sun = btn.dataset.s; rebuild();
    });
    document.getElementById('scopeSeg').addEventListener('click', e => {
      const btn = e.target.closest('button'); if (!btn) return;
      [...e.currentTarget.children].forEach(b => b.classList.toggle('on', b === btn));
      state.only7 = btn.dataset.k === '7'; rebuild();
    });
    bindToggle('swAnim', 'anim', () => updateHud());
    bindToggle('swGrid', 'grid');
    bindToggle('swOutline', 'outline');
    fit(); rebuild();
    window.__polisMon = { get gallery() { return galleryC; }, get world() { return world; }, state, rebuild, centerView };
  }

  if (document.fonts && document.fonts.ready) {
    document.fonts.ready.then(init);
    setTimeout(() => { if (!galleryC) init(); }, 1200);
  } else { init(); }
})();
