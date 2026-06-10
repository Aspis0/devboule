/* =========================================================================
   map_app.js — Polis diorama map (PixiJS v7)
   -------------------------------------------------------------------------
   A hand-composed isometric scene: grass terrain, two inland rivers with
   bridges, a sea with a harbour on the shore, paved roads, a cluster of
   town buildings and one Wonder (Meraviglia). Citizens walk ONLY on road
   and bridge tiles — never on water or buildings.
   ========================================================================= */
(function () {
  'use strict';
  const M = ISO.MAT, S = ISO.shade;
  const N = 18;                          // grid size
  const SEA_Y = 14;                      // gy >= SEA_Y is open sea
  const RIVERS = [{ gx: [12, 13] }, { gx: [3, 3] }]; // two inland rivers, flow gy 0..SEA_Y

  const canvas = document.getElementById('stage');
  const wrap = document.getElementById('stagewrap');
  const state = { anim: true, grid: false, sun: 'NW' };

  const app = new PIXI.Application({
    view: canvas, antialias: true, resolution: Math.min(2, window.devicePixelRatio || 1),
    autoDensity: true, backgroundColor: 0xC9D4D4, preserveDrawingBuffer: true
  });
  const bg = new PIXI.Graphics(); app.stage.addChildAt(bg, 0);
  function paintBg(w, h) {
    bg.clear();
    bg.beginFill(0xCFD9D8); bg.drawRect(0, 0, w, h); bg.endFill();
    bg.beginFill(0xE7E0CF, 0.55); bg.drawRect(0, 0, w, h * 0.5); bg.endFill();
  }
  const world = new PIXI.Container(); app.stage.addChild(world);
  let activeAnims = [], T = 0;
  const proj = ISO.makeProj(N, N);

  // ---- road / plaza / bridge network (authored) --------------------------
  const roadSet = new Set(), plazaSet = new Set(), bridgeSet = new Set();
  const key = (x, y) => x + '|' + y;
  function seg(x0, y0, x1, y1, set) {
    const dx = Math.sign(x1 - x0), dy = Math.sign(y1 - y0);
    let x = x0, y = y0; set.add(key(x, y));
    while (x !== x1 || y !== y1) { x += dx; y += dy; set.add(key(x, y)); }
  }
  seg(1, 3, 16, 3, roadSet);     // H1 north street
  seg(1, 8, 16, 8, roadSet);     // H2 main street
  seg(4, 12, 10, 12, roadSet);   // H3 shore street (behind harbour)
  seg(5, 3, 5, 12, roadSet);     // V1 left avenue → shore
  seg(10, 3, 10, 12, roadSet);   // V2 right avenue → shore
  seg(1, 3, 1, 8, roadSet);      // V0 west spur
  seg(16, 3, 16, 8, roadSet);    // V3 east-bank spur
  // bridges where streets cross the rivers
  [[3, 3], [3, 8], [12, 3], [13, 3], [12, 8], [13, 8]].forEach(b => bridgeSet.add(key(b[0], b[1])));
  // civic plaza around the Wonder
  for (let x = 6; x <= 9; x++) for (let y = 4; y <= 7; y++) plazaSet.add(key(x, y));

  function isRiver(gx, gy) { return gy < SEA_Y && RIVERS.some(r => gx >= r.gx[0] && gx <= r.gx[1]); }
  function landType(gx, gy) {
    if (roadSet.has(key(gx, gy))) return 'road';
    if (plazaSet.has(key(gx, gy))) return 'plaza';
    if (gy === SEA_Y - 1) return 'sand';
    if (RIVERS.some(r => gx === r.gx[0] - 1 || gx === r.gx[1] + 1)) return 'sand'; // banks
    return 'grass';
  }

  // ---- terrain render ----------------------------------------------------
  function drawTile(g, gx, gy, type, grid) {
    const a = proj.p(gx, gy, 0), b = proj.p(gx + 1, gy, 0), c = proj.p(gx + 1, gy + 1, 0), d = proj.p(gx, gy + 1, 0);
    const v = ((gx * 7 + gy * 13) % 5) / 5;
    let col, edge = M.groundEdge;
    if (type === 'grass') col = S(ISO.mix(M.ground, (gx + gy) % 2 ? M.grassDk : M.earth, 0.2 + v * 0.14), 1);
    else if (type === 'sand') col = S(M.sand, 0.96 + v * 0.08);
    else if (type === 'road') { col = S(M.stone, 0.95 + v * 0.05); edge = S(M.stone, 0.74); }
    else if (type === 'plaza') { col = S(M.stone, 1.06 + v * 0.04); edge = S(M.stone, 0.82); }
    else return;
    ISO.poly(g, [a, b, c, d], col);
    if (type === 'road' || type === 'plaza') ISO.outlinePoly(g, [a, b, c, d], edge, 1, 0.55);
    else if (grid) ISO.outlinePoly(g, [a, b, c, d], edge, 1, 0.22);
    else if (type === 'grass' && v > 0.55) {
      const cx = (a.x + c.x) / 2, cy = (a.y + c.y) / 2;
      g.lineStyle({ width: 1, color: M.grass, alpha: 0.7 });
      for (let k = -1; k <= 1; k++) { g.moveTo(cx + k * 4 + v * 5, cy + 4); g.lineTo(cx + k * 4 + v * 5 - 1, cy - 1); }
      g.lineStyle(0);
    }
  }
  // wooden bridge deck spanning one tile, raised over the water
  function drawBridge(g, gx, gy) {
    ISO.box(g, proj, gx + 0.02, gy + 0.02, 0.06, 0.96, 0.96, 0.12, M.wood, { tex: 'wood', outline: true, outlineAlpha: 0.25 });
    // plank seams on the deck top
    const z = 0.18;
    g.lineStyle({ width: 1, color: S(M.wood, 0.7), alpha: 0.5 });
    for (let i = 1; i < 5; i++) { const t = i / 5; const a = proj.p(gx + 0.02, gy + 0.02 + t * 0.96, z), b = proj.p(gx + 0.98, gy + 0.02 + t * 0.96, z); g.moveTo(a.x, a.y); g.lineTo(b.x, b.y); }
    g.lineStyle(0);
    // rail posts
    [[0.05, 0.05], [0.05, 0.95], [0.95, 0.05], [0.95, 0.95]].forEach(p => {
      const q = proj.p(gx + p[0], gy + p[1], 0.18); g.beginFill(S(M.woodDk, 1)); g.drawRect(q.x - 1, q.y - 5, 2, 5); g.endFill();
    });
  }

  // ---- animated water over an arbitrary tile set -------------------------
  function makeWater(tiles) {
    const node = new PIXI.Container();
    const mask = new PIXI.Graphics(), base = new PIXI.Graphics(), g = new PIXI.Graphics();
    let minx = 1e9, maxx = -1e9, miny = 1e9, maxy = -1e9;
    tiles.forEach(t => {
      const a = proj.p(t.gx, t.gy, 0), b = proj.p(t.gx + 1, t.gy, 0), c = proj.p(t.gx + 1, t.gy + 1, 0), d = proj.p(t.gx, t.gy + 1, 0);
      const flat = [a.x, a.y, b.x, b.y, c.x, c.y, d.x, d.y];
      mask.beginFill(0xffffff); mask.drawPolygon(flat); mask.endFill();
      base.beginFill(t.deep ? M.waterDeep : M.water); base.drawPolygon(flat); base.endFill();
      [a, b, c, d].forEach(p => { minx = Math.min(minx, p.x); maxx = Math.max(maxx, p.x); miny = Math.min(miny, p.y); maxy = Math.max(maxy, p.y); });
    });
    node.addChild(base); node.addChild(g); node.addChild(mask); g.mask = mask;
    return {
      node, t: Math.random() * 5,
      update(_t, dt) {
        this.t += dt; g.clear();
        const rows = Math.max(3, Math.round((maxy - miny) / 9));
        for (let r = 0; r < rows; r++) {
          const yy = miny + (r / rows) * (maxy - miny), off = Math.sin(this.t * 1.8 + r * 0.8) * 5;
          g.lineStyle({ width: 1.4, color: S(M.water, 1.34), alpha: 0.4 });
          g.moveTo(minx, yy + off);
          for (let x = minx; x <= maxx; x += 11) g.lineTo(x, yy + off + Math.sin(this.t * 2.6 + x * 0.05) * 2);
          g.lineStyle(0);
        }
      }
    };
  }

  // ---- scene contents ----------------------------------------------------
  const WONDER = { kind: 'MON', type: 'mausoleion', gx: 6, gy: 4 };
  const BUILDINGS = [
    { type: 'temple', L: 1, gx: 6, gy: 0 },
    { type: 'market', L: 1, gx: 8, gy: 0 },
    { type: 'house', L: 2, gx: 0, gy: 1 },
    { type: 'townhall', L: 1, gx: 6, gy: 9 },
    { type: 'house', L: 1, gx: 9, gy: 9 },
    { type: 'house', L: 1, gx: 9, gy: 10 },
    { type: 'warehouse', L: 1, gx: 0, gy: 9 },
    { type: 'house', L: 2, gx: 14, gy: 4 },
    { type: 'baths', L: 1, gx: 14, gy: 9 },
    { type: 'tower', L: 2, gx: 15, gy: 1 },
    { type: 'harbor', L: 2, gx: 6, gy: 13 },
    { type: 'lighthouse', L: 3, gx: 10, gy: 13 }
  ];

  // ---- people paths (road + bridge tiles ONLY) ---------------------------
  const center = (gx, gy) => proj.p(gx + 0.5, gy + 0.5, 0.05);
  const pathFrom = tiles => tiles.map(t => center(t[0], t[1]));
  const LOOPS = [
    pathFrom([[5, 3], [10, 3], [10, 8], [5, 8]]),                       // civic block
    pathFrom([[5, 8], [5, 12], [10, 12], [10, 8]]),                     // shore block
    pathFrom([[1, 3], [1, 8], [5, 8], [5, 3]]),                         // west loop (bridge B @3,3)
    pathFrom([[10, 3], [13, 3], [16, 3], [16, 8], [13, 8], [10, 8]]),   // east loop (bridges A)
    pathFrom([[5, 3], [16, 3], [16, 8], [5, 8]])                        // long ring over the bridges
  ];

  // ======================================================================
  let sceneC = null;
  function build() {
    if (sceneC) sceneC.destroy({ children: true });
    activeAnims = [];
    sceneC = new PIXI.Container(); world.addChild(sceneC);
    ISO.SUN.dir = state.sun;

    // 1) terrain land + collect water tiles (rivers continue UNDER bridges)
    const land = new PIXI.Graphics(); sceneC.addChild(land);
    const waterTiles = [];
    for (let s = 0; s <= 2 * (N - 1); s++) {
      for (let gx = 0; gx < N; gx++) {
        const gy = s - gx; if (gy < 0 || gy >= N) continue;
        if (gy >= SEA_Y) { waterTiles.push({ gx, gy, deep: gy >= SEA_Y + 1 }); }
        else if (isRiver(gx, gy)) { waterTiles.push({ gx, gy, deep: false }); }
        else drawTile(land, gx, gy, landType(gx, gy), state.grid);
      }
    }
    ISO.outlinePoly(land, [proj.p(0, 0, 0), proj.p(N, 0, 0), proj.p(N, N, 0), proj.p(0, N, 0)], S(M.groundEdge, 0.9), 1.5, 0.5);

    // 2) animated water (sea + rivers)
    const water = makeWater(waterTiles); sceneC.addChild(water.node); activeAnims.push(water);

    // 3) bridge decks over the rivers (back→front)
    const bridges = new PIXI.Graphics(); sceneC.addChild(bridges);
    [...bridgeSet].map(k => k.split('|').map(Number)).sort((a, b) => (a[0] + a[1]) - (b[0] + b[1]))
      .forEach(([gx, gy]) => drawBridge(bridges, gx, gy));

    // 4) depth-sorted objects (wonder + buildings)
    const objs = [WONDER, ...BUILDINGS].map(o => {
      const lib = o.kind === 'MON' ? window.MON : window.BUILD;
      const res = o.kind === 'MON' ? lib[o.type]({ outline: false }) : lib[o.type](o.L, { outline: false });
      return { o, res, W: res.foot[0], D: res.foot[1] };
    });
    objs.sort((a, b) => (a.o.gx + a.W + a.o.gy + a.D) - (b.o.gx + b.W + b.o.gy + b.D));
    objs.forEach(({ o, res, W, D }) => {
      const sh = new PIXI.Graphics();
      const cc = proj.p(o.gx + W / 2, o.gy + D / 2, 0);
      sh.beginFill(0x2a2a25, 0.12); sh.drawEllipse(cc.x, cc.y, (W + D) * ISO.HALF_W * 0.4, (W + D) * ISO.HALF_H * 0.4); sh.endFill();
      sceneC.addChild(sh);
      const at = proj.p(o.gx + W, o.gy + D, 0);
      res.container.position.set(at.x, at.y);
      sceneC.addChild(res.container);
      res.anims.forEach(a => activeAnims.push(a));
    });

    // 5) citizens — walk roads + bridges only (drawn on top)
    const peopleC = new PIXI.Container(); sceneC.addChild(peopleC);
    const cast = [
      ['citizen', 0], ['merchant', 1], ['watercarrier', 2], ['noble', 3],
      ['citizen', 4], ['watercarrier', 1], ['merchant', 3], ['noble', 0], ['citizen', 4]
    ];
    cast.forEach(([type, li], i) => {
      const path = LOOPS[li];
      const p = PEOPLE.make(type, { scale: 1, path, speed: 12 + (i % 4) * 4 });
      p.seg = i % path.length; p.u = (i * 0.37) % 1;
      peopleC.addChild(p.node); activeAnims.push(p);
    });

    updateHud();
  }

  function centerView() {
    if (!sceneC) return;
    const vw = wrap.clientWidth, vh = wrap.clientHeight, b = sceneC.getLocalBounds(), m = 60;
    const sc = Math.min((vw - m) / b.width, (vh - m) / b.height, 1.2);
    world.scale.set(sc);
    world.position.set(vw / 2 - (b.x + b.width / 2) * sc, vh / 2 - (b.y + b.height / 2) * sc);
    updateHud();
  }
  function fit() { const w = wrap.clientWidth, h = wrap.clientHeight; app.renderer.resize(w, h); paintBg(w, h); }
  window.addEventListener('resize', () => { fit(); centerView(); });

  let dragging = false, last = null;
  canvas.addEventListener('pointerdown', e => { dragging = true; last = { x: e.clientX, y: e.clientY }; });
  window.addEventListener('pointermove', e => { if (!dragging) return; world.position.x += e.clientX - last.x; world.position.y += e.clientY - last.y; last = { x: e.clientX, y: e.clientY }; });
  window.addEventListener('pointerup', () => { dragging = false; });
  canvas.addEventListener('wheel', e => {
    e.preventDefault();
    const r = canvas.getBoundingClientRect(), mx = e.clientX - r.left, my = e.clientY - r.top;
    const factor = e.deltaY < 0 ? 1.12 : 1 / 1.12;
    const pre = { x: (mx - world.x) / world.scale.x, y: (my - world.y) / world.scale.y };
    const ns = Math.max(0.3, Math.min(3, world.scale.x * factor));
    world.scale.set(ns); world.position.set(mx - pre.x * ns, my - pre.y * ns); updateHud();
  }, { passive: false });

  app.ticker.add(() => {
    const dt = state.anim ? Math.min(0.05, app.ticker.deltaMS / 1000) : 0;
    T += dt; for (const a of activeAnims) if (a && a.update) a.update(T, dt);
  });

  function updateHud() {
    const hud = document.getElementById('hud');
    const z = Math.round(world.scale.x * 100);
    hud.innerHTML = '<b>Mappa</b> ' + N + '×' + N + ' · ' + BUILDINGS.length + ' edifici · 1 meraviglia · 2 fiumi + mare · zoom ' + z + '%';
  }

  function bindToggle(id, k, cb) { const el = document.getElementById(id); el.addEventListener('click', () => { state[k] = !state[k]; el.classList.toggle('on', state[k]); (cb || rebuild)(); }); }
  function rebuild() { build(); centerView(); }

  let _inited = false;
  function init() {
    if (_inited) return; _inited = true;
    document.getElementById('sunSeg').addEventListener('click', e => {
      const btn = e.target.closest('button'); if (!btn) return;
      [...e.currentTarget.children].forEach(b => b.classList.toggle('on', b === btn));
      state.sun = btn.dataset.s; rebuild();
    });
    bindToggle('swAnim', 'anim', () => updateHud());
    bindToggle('swGrid', 'grid');
    fit(); rebuild();
    window.__polisMap = { get scene() { return sceneC; }, get world() { return world; }, state, rebuild, centerView };
  }
  if (document.fonts && document.fonts.ready) { document.fonts.ready.then(init); setTimeout(() => { if (!sceneC) init(); }, 1200); }
  else init();
})();
