/* =========================================================================
   people.js — Procedural Greek citizens (PixiJS v7) — AESTHETIC ONLY
   -------------------------------------------------------------------------
   Small animated figures drawn in screen-space (billboards). Each has a
   .node (PIXI.Container) and .update(t, dt). Behaviours:
     - path-follow (walk a list of screen points, loop)
     - range-oscillate (pace back and forth) for showcase cells
     - in-place action: builder hammers, firefighter throws water
   Types: citizen (Polites), builder (Tekton), firefighter (Pyrosbestes),
          watercarrier (Hydrophoros), merchant (Emporos), noble (Eupatrides)
   These are visuals only — no gameplay/AI. Wire your own pathing to setPath.
   ========================================================================= */
(function (global) {
  'use strict';
  const S = (h, f) => ISO.shade(h, f);
  const SK = 0xCB9F6E, SKd = 0xA67C4C, HAIR = 0x35261A;

  const TUNIC = {
    citizen: 0xE6DCC4, builder: 0x8A6234, firefighter: 0xB23A30,
    watercarrier: 0xBFC7CC, merchant: 0xC98A2B, noble: 0xF3EDDE
  };

  const ORDER = ['citizen', 'builder', 'firefighter', 'watercarrier', 'merchant', 'noble'];
  const INFO = {
    citizen: { name: 'Polites', it: 'cittadino' },
    builder: { name: 'Tekton', it: 'costruttore · martello' },
    firefighter: { name: 'Pyrosbestes', it: 'spegne le fiamme' },
    watercarrier: { name: 'Hydrophoros', it: "portatore d'acqua" },
    merchant: { name: 'Emporos', it: 'mercante' },
    noble: { name: 'Eupatrides', it: 'nobile' }
  };

  function limb(g, ax, ay, bx, by, w, col) {
    g.lineStyle({ width: w, color: col, cap: 'round' });
    g.moveTo(ax, ay); g.lineTo(bx, by); g.lineStyle(0);
  }

  class Person {
    constructor(type, opts) {
      opts = opts || {};
      this.type = type;
      this.scale = opts.scale || 1;
      this.tunic = opts.tunic || TUNIC[type] || 0xE6DCC4;
      this.node = new PIXI.Container();
      this.g = new PIXI.Graphics();
      this.node.addChild(this.g);
      this.t = Math.random() * 10;
      this.walkPhase = Math.random() * 6;
      this.face = opts.face || 1;
      this.speed = opts.speed || (type === 'noble' ? 13 : type === 'builder' ? 0 : 20);
      this.path = opts.path || null; this.seg = 0; this.u = 0; this.loop = opts.loop !== false;
      this.range = opts.range || null; this.dir = 1;
      if (this.range) this.node.position.set(opts.x !== undefined ? opts.x : (this.range.x0 + this.range.x1) / 2, this.range.y || 0);
      else if (opts.x !== undefined) this.node.position.set(opts.x, opts.y || 0);
      if (this.path) this.node.position.set(this.path[0].x, this.path[0].y);
      this.kind = 'person';
    }

    _follow(dt) {
      if (this.path.length < 2) return false;
      const a = this.path[this.seg], b = this.path[(this.seg + 1) % this.path.length];
      const len = Math.hypot(b.x - a.x, b.y - a.y) || 1;
      this.u += (this.speed * dt) / len;
      while (this.u >= 1) { this.u -= 1; this.seg = (this.seg + 1) % this.path.length; }
      const a2 = this.path[this.seg], b2 = this.path[(this.seg + 1) % this.path.length];
      const x = a2.x + (b2.x - a2.x) * this.u, y = a2.y + (b2.y - a2.y) * this.u;
      if (Math.abs(b2.x - a2.x) > 0.1) this.face = b2.x > a2.x ? 1 : -1;
      this.node.position.set(x, y);
      return true;
    }
    _osc(dt) {
      const r = this.range; let x = this.node.position.x + this.speed * dt * this.dir;
      if (x > r.x1) { x = r.x1; this.dir = -1; } else if (x < r.x0) { x = r.x0; this.dir = 1; }
      this.face = this.dir; this.node.position.set(x, r.y || 0);
      return true;
    }

    update(t, dt) {
      let moving = false;
      if (this.path) moving = this._follow(dt);
      else if (this.range) moving = this._osc(dt);
      if (moving) this.walkPhase += dt * 9;
      this.t += dt;
      this.node.scale.x = this.face;
      const hammer = this.type === 'builder';
      const extinguish = this.type === 'firefighter';
      this._draw(moving, hammer ? this.t * 5 : 0, extinguish);
    }

    _draw(moving, hammerPhase, extinguish) {
      const g = this.g, s = this.scale; g.clear();
      const tunic = this.tunic, tDk = S(tunic, 0.78);
      const sw = moving ? Math.sin(this.walkPhase) : 0;
      const hipY = -7.5 * s, shY = -15 * s;
      // shadow
      g.beginFill(0x241a10, 0.16); g.drawEllipse(0, 0, 6 * s, 2.2 * s); g.endFill();
      // legs (front opposite to back)
      limb(g, 0.4 * s, hipY, sw * 2.6 * s, 0, 2.3 * s, SKd);
      limb(g, -0.4 * s, hipY, -sw * 2.6 * s, 0, 2.5 * s, SK);
      g.beginFill(0x4A3320); g.drawEllipse(sw * 2.6 * s, 0, 2 * s, 1 * s); g.drawEllipse(-sw * 2.6 * s, 0, 2 * s, 1 * s); g.endFill();
      // tunic
      g.beginFill(tunic); g.drawPolygon([-3.6 * s, hipY + 0.6 * s, 3.6 * s, hipY + 0.6 * s, 2.6 * s, -16.4 * s, -2.6 * s, -16.4 * s]); g.endFill();
      g.beginFill(tDk, 0.5); g.drawPolygon([0.3 * s, hipY + 0.6 * s, 3.6 * s, hipY + 0.6 * s, 2.6 * s, -16.4 * s, 0.3 * s, -16.4 * s]); g.endFill();
      g.lineStyle({ width: 1.1 * s, color: this.type === 'noble' ? 0xC9A03A : tDk }); g.moveTo(-3 * s, -9.2 * s); g.lineTo(3 * s, -9.2 * s); g.lineStyle(0);
      // back accessories (behind body) — merchant sack
      if (this.type === 'merchant') {
        g.beginFill(0xB89A5C); g.drawEllipse(-2.8 * s, -13.5 * s, 3 * s, 3.8 * s); g.endFill();
        g.beginFill(S(0xB89A5C, 0.82)); g.drawEllipse(-3.4 * s, -12.4 * s, 1.3 * s, 2.4 * s); g.endFill();
      }
      // arms
      if (this.type === 'builder') {
        limb(g, -2.7 * s, shY, -3.3 * s, -8.6 * s, 2 * s, SKd);
        const a = -1.2 + Math.sin(hammerPhase) * 1.0;
        const sx = 2.7 * s, sy = shY, ex = sx + Math.cos(a) * 7 * s, ey = sy + Math.sin(a) * 7 * s;
        limb(g, sx, sy, ex, ey, 2 * s, SK);
        const hx = ex + Math.cos(a) * 4 * s, hy = ey + Math.sin(a) * 4 * s;
        g.lineStyle({ width: 1.5 * s, color: 0x6E4A2A, cap: 'round' }); g.moveTo(ex, ey); g.lineTo(hx, hy); g.lineStyle(0);
        g.beginFill(0x55555E); g.drawRect(hx - 2.3 * s, hy - 2 * s, 4.6 * s, 3.1 * s); g.endFill();
        g.beginFill(S(0x55555E, 1.25)); g.drawRect(hx - 2.3 * s, hy - 2 * s, 4.6 * s, 1 * s); g.endFill();
        if (Math.sin(hammerPhase) > 0.86) { g.beginFill(0xFFE6A0, 0.9); for (let k = 0; k < 4; k++) { const ka = k * 1.6; g.drawCircle(hx + Math.cos(ka) * 3 * s, hy + 2.4 * s + Math.sin(ka) * 2 * s, 0.9 * s); } g.endFill(); }
      } else {
        const aSw = sw * 2.4 * s;
        limb(g, -2.7 * s, shY, -3.3 * s - aSw, -8.6 * s, 2 * s, SKd);
        if (this.type === 'firefighter') {
          // front arm holds a bucket
          limb(g, 2.7 * s, shY, 3.7 * s, -10 * s, 2 * s, SK);
          const bx = 4.4 * s, by = -8.6 * s;
          g.beginFill(0x6E4A2A); g.drawRect(bx - 2.1 * s, by - 3 * s, 4.2 * s, 4.2 * s); g.endFill();
          g.beginFill(S(0x3C7B92, 1.15)); g.drawRect(bx - 1.7 * s, by - 3 * s, 3.4 * s, 1.5 * s); g.endFill();
          g.lineStyle({ width: 0.9 * s, color: 0x4A3320 }); g.moveTo(bx - 2.1 * s, by - 3 * s); g.lineTo(bx + 2.1 * s, by - 3 * s); g.lineStyle(0);
        } else {
          limb(g, 2.7 * s, shY, 3.3 * s + aSw, -8.6 * s, 2 * s, SK);
        }
      }
      // water-carrier yoke + amphorae (over shoulders)
      if (this.type === 'watercarrier') {
        g.lineStyle({ width: 1.5 * s, color: 0x6E4A2A, cap: 'round' }); g.moveTo(-6.2 * s, shY - 0.5 * s); g.lineTo(6.2 * s, shY - 0.5 * s); g.lineStyle(0);
        [-6, 6].forEach(x => {
          g.lineStyle({ width: 0.8 * s, color: 0x4A3320 }); g.moveTo(x * s, shY - 0.3 * s); g.lineTo(x * s, -11 * s); g.lineStyle(0);
          g.beginFill(S(0xC0613A, 0.95)); g.drawEllipse(x * s, -9 * s, 2.4 * s, 3.6 * s); g.endFill();
          g.beginFill(S(0xC0613A, 1.15)); g.drawEllipse(x * s - 0.8 * s, -9.8 * s, 0.9 * s, 2 * s); g.endFill();
        });
      }
      // head
      g.beginFill(HAIR); g.drawCircle(0, -19.3 * s, 2.95 * s); g.endFill();
      g.beginFill(SK); g.drawCircle(0, -18.3 * s, 2.6 * s); g.endFill();
      // noble himation cloak over one shoulder
      if (this.type === 'noble') {
        g.beginFill(0xEFE7D2); g.drawPolygon([-4 * s, hipY, 1.4 * s, hipY, 2.4 * s, -14.5 * s, -1.6 * s, -16 * s, -4.6 * s, -11 * s]); g.endFill();
        g.lineStyle({ width: 1 * s, color: 0x7A3F86, alpha: 0.85 }); g.moveTo(-4.6 * s, -11 * s); g.lineTo(-4 * s, hipY); g.lineStyle(0);
        // staff
        g.lineStyle({ width: 1.1 * s, color: 0x6E4A2A }); g.moveTo(3.6 * s, -17 * s); g.lineTo(3.6 * s, 0); g.lineStyle(0);
      }
      // firefighter water throw arc
      if (extinguish) {
        const ph = (this.t * 0.9) % 1;
        if (ph < 0.45) {
          g.beginFill(0x7CC0DA, 0.85);
          for (let k = 0; k < 6; k++) { const tt = k / 5; const px = (5 + tt * 13) * s; const py = -11 * s - Math.sin(tt * Math.PI) * 9 * s; g.drawCircle(px, py, (1.4 - tt * 0.5) * s); }
          g.endFill();
        }
      }
    }
  }

  function make(type, opts) { return new Person(type, opts); }

  global.PEOPLE = { Person, make, order: ORDER, info: INFO, TUNIC };
})(window);
