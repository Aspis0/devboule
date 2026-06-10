/* =========================================================================
   anims.js — Separately-animatable parts for buildings (PixiJS v7)
   -------------------------------------------------------------------------
   Each part is a small class that owns a PIXI.Container ("node") added on
   top of the static building body, plus an update(t, dt) called by the app
   ticker. Toggling animation off simply stops update() and can hide nodes.
   Anchoring is done in screen-space via a projected attach point.
   ========================================================================= */
(function (global) {
  'use strict';
  const M = ISO.MAT;
  const rand = (a, b) => a + Math.random() * (b - a);

  // ---- Flame / brazier ---------------------------------------------------
  class Flame {
    constructor(x, y, scale) {
      this.node = new PIXI.Container();
      this.node.position.set(x, y);
      this.s = scale || 1;
      this.glow = new PIXI.Graphics(); this.node.addChild(this.glow);
      this.g = new PIXI.Graphics(); this.node.addChild(this.g);
      this.t = Math.random() * 10;
      this.kind = 'flame';
    }
    update(t, dt) {
      this.t += dt;
      const s = this.s, fl = 0.9 + Math.sin(this.t * 11) * 0.14 + Math.sin(this.t * 23) * 0.06;
      const sway = Math.sin(this.t * 7) * 2.6 * s, sway2 = Math.sin(this.t * 13 + 1) * 2 * s;
      const g = this.g; g.clear();
      // embers base
      g.beginFill(0xB23A1E, 0.9); g.drawEllipse(0, -4 * s, 8.5 * s, 5 * s); g.endFill();
      // outer flame (taller, fuller)
      g.beginFill(0xE8541F, 0.96);
      g.drawEllipse(sway, -15 * s * fl, 9 * s, 19 * s * fl); g.endFill();
      // secondary tongue
      g.beginFill(0xF2731F, 0.9);
      g.drawEllipse(sway2 + 3 * s, -13 * s * fl, 4.5 * s, 13 * s * fl); g.endFill();
      // mid
      g.beginFill(0xF7A024, 0.97);
      g.drawEllipse(sway * 0.7, -16 * s * fl, 5.6 * s, 14 * s * fl); g.endFill();
      // core
      g.beginFill(0xFFE7A0, 1);
      g.drawEllipse(sway * 0.5, -14 * s * fl, 3 * s, 9 * s * fl); g.endFill();
      g.beginFill(0xFFF6DA, 1);
      g.drawEllipse(sway * 0.4, -11 * s * fl, 1.6 * s, 5 * s * fl); g.endFill();
      // glow
      const gl = this.glow; gl.clear();
      gl.beginFill(0xF2922E, 0.2 + 0.07 * Math.sin(this.t * 9));
      gl.drawCircle(0, -11 * s, 30 * s); gl.endFill();
    }
  }

  // ---- Beacon (lighthouse) — rotating light + pulsing core ---------------
  class Beacon {
    constructor(x, y, scale) {
      this.node = new PIXI.Container();
      this.node.position.set(x, y);
      this.s = scale || 1;
      this.beam = new PIXI.Graphics(); this.node.addChild(this.beam);
      this.core = new PIXI.Graphics(); this.node.addChild(this.core);
      this.t = Math.random() * 6;
      this.kind = 'beacon';
    }
    update(t, dt) {
      this.t += dt;
      const s = this.s;
      const ang = this.t * 1.4;
      const b = this.beam; b.clear();
      // two opposed beams sweeping (in iso, widen horizontally)
      for (const dir of [0, Math.PI]) {
        const a = ang + dir;
        const dx = Math.cos(a), len = 120 * s, spread = 26 * s;
        const hx = dx * len, hy = -Math.abs(Math.sin(a)) * 8 * s - 6 * s;
        b.beginFill(0xFFE7A0, 0.16);
        b.drawPolygon([0, -4 * s, hx - spread * 0.3, hy - spread, hx + spread * 0.3, hy + spread]);
        b.endFill();
      }
      const pulse = 0.7 + Math.sin(this.t * 6) * 0.3;
      const c = this.core; c.clear();
      c.beginFill(0xFFF0C0, 0.3 + 0.2 * pulse); c.drawCircle(0, -5 * s, 13 * s * pulse); c.endFill();
      c.beginFill(0xFFD45A, 1); c.drawCircle(0, -5 * s, 5.2 * s); c.endFill();
      c.beginFill(0xFFFBEC, 1); c.drawCircle(0, -5 * s, 2.4 * s); c.endFill();
    }
  }

  // ---- Flag / banner waving ----------------------------------------------
  class Flag {
    constructor(x, y, scale, color) {
      this.node = new PIXI.Container();
      this.node.position.set(x, y);
      this.s = scale || 1;
      this.color = color || M.red;
      this.pole = new PIXI.Graphics(); this.node.addChild(this.pole);
      this.g = new PIXI.Graphics(); this.node.addChild(this.g);
      this.t = Math.random() * 8;
      this.kind = 'flag';
      this._pole();
    }
    _pole() {
      const s = this.s, p = this.pole;
      p.beginFill(M.wood); p.drawRect(-1.2 * s, -36 * s, 2.4 * s, 36 * s); p.endFill();
      p.beginFill(M.gold); p.drawCircle(0, -36 * s, 2.6 * s); p.endFill();
    }
    update(t, dt) {
      this.t += dt;
      const s = this.s, g = this.g; g.clear();
      const top = -34 * s, h = 13 * s, len = 26 * s, segs = 8;
      const pts = [];
      for (let i = 0; i <= segs; i++) {
        const u = i / segs;
        const wav = Math.sin(this.t * 6 - u * 5) * 3.2 * s * u;
        pts.push({ x: u * len, y: top + wav });
      }
      const bot = [];
      for (let i = segs; i >= 0; i--) {
        const u = i / segs;
        const wav = Math.sin(this.t * 6 - u * 5) * 3.2 * s * u;
        bot.push({ x: u * len, y: top + h + wav });
      }
      const all = pts.concat(bot);
      const flat = []; all.forEach(p => flat.push(p.x, p.y));
      g.beginFill(this.color, 1); g.drawPolygon(flat); g.endFill();
      // shaded lower third
      const flat2 = [];
      pts.forEach(p => flat2.push(p.x, p.y + h * 0.62));
      bot.forEach(p => flat2.push(p.x, p.y));
      g.beginFill(ISO.shade(this.color, 0.8), 1); g.drawPolygon(flat2); g.endFill();
    }
  }

  // ---- Smoke — rising fading puffs ---------------------------------------
  class Smoke {
    constructor(x, y, scale) {
      this.node = new PIXI.Container();
      this.node.position.set(x, y);
      this.s = scale || 1;
      this.g = new PIXI.Graphics(); this.node.addChild(this.g);
      this.puffs = [];
      this.acc = 0; this.t = 0;
      this.kind = 'smoke';
    }
    update(t, dt) {
      this.t += dt; this.acc += dt;
      const s = this.s;
      if (this.acc > 0.28) { this.acc = 0; this.puffs.push({ life: 0, x: rand(-2, 2) * s, drift: rand(-7, 10) * s, r0: rand(3, 5) }); }
      const g = this.g; g.clear();
      for (let i = this.puffs.length - 1; i >= 0; i--) {
        const p = this.puffs[i]; p.life += dt * 0.42;
        if (p.life > 1) { this.puffs.splice(i, 1); continue; }
        const y = -p.life * 52 * s, x = p.x + p.drift * p.life;
        const r = (p.r0 + p.life * 13) * s, a = 0.5 * (1 - p.life);
        g.beginFill(0x7E7868, a); g.drawCircle(x, y, r); g.endFill();
        g.beginFill(0x9C968A, a * 0.8); g.drawCircle(x - r * 0.25, y - r * 0.22, r * 0.62); g.endFill();
        g.beginFill(0xB6B0A2, a * 0.5); g.drawCircle(x - r * 0.4, y - r * 0.35, r * 0.34); g.endFill();
      }
    }
  }

  // ---- Water — animated ripples over a polygon region --------------------
  class Water {
    // pts: array of {x,y} screen-space polygon (the basin / harbor surface)
    constructor(pts, scale) {
      this.node = new PIXI.Container();
      this.s = scale || 1;
      this.pts = pts;
      this.mask = new PIXI.Graphics();
      const flat = []; pts.forEach(p => flat.push(p.x, p.y));
      this.mask.beginFill(0xffffff); this.mask.drawPolygon(flat); this.mask.endFill();
      this.base = new PIXI.Graphics();
      this.base.beginFill(M.water); this.base.drawPolygon(flat); this.base.endFill();
      this.g = new PIXI.Graphics();
      this.node.addChild(this.base); this.node.addChild(this.g);
      this.node.addChild(this.mask); this.g.mask = this.mask;
      // bounds
      let minx = 1e9, maxx = -1e9, miny = 1e9, maxy = -1e9;
      pts.forEach(p => { minx = Math.min(minx, p.x); maxx = Math.max(maxx, p.x); miny = Math.min(miny, p.y); maxy = Math.max(maxy, p.y); });
      this.b = { minx, maxx, miny, maxy };
      this.t = Math.random() * 5;
      this.kind = 'water';
    }
    update(t, dt) {
      this.t += dt;
      const g = this.g; g.clear();
      const { minx, maxx, miny, maxy } = this.b, s = this.s;
      const rows = Math.max(3, Math.round((maxy - miny) / (7 * s)));
      for (let r = 0; r < rows; r++) {
        const yy = miny + (r / rows) * (maxy - miny);
        const off = Math.sin(this.t * 2 + r * 0.9) * 6 * s;
        g.lineStyle({ width: 1.6 * s, color: ISO.shade(M.water, 1.32), alpha: 0.5 });
        g.moveTo(minx, yy + off);
        for (let x = minx; x <= maxx; x += 10 * s) {
          g.lineTo(x, yy + off + Math.sin(this.t * 3 + x * 0.06) * 2 * s);
        }
      }
      g.lineStyle(0);
    }
  }

  global.ANIM = { Flame, Beacon, Flag, Smoke, Water };
})(window);
