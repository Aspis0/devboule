/* =========================================================================
   anims.ts — Separately-animatable building parts (faithful port of anims.js)
   -------------------------------------------------------------------------
   Each part owns a Container ("node") added on top of the static building body,
   plus an update(t, dt) called by the renderer's step clock. Anchoring is done
   in screen-space via a projected attach point.

   PORTED 1:1 from Polis-handoff/polis/project/js/anims.js (PixiJS v7) → v8/TS.
   The per-frame clear()+redraw of each part's small Graphics is INHERENT to the
   source art (that is how the flicker/wave/puff are produced) and is kept; the
   adapter gates these updates to VISIBLE chunks only.

   DETERMINISM: animation randomness (flicker phase, smoke jitter, flag wave)
   is TIME-BASED and not part of the deterministic city state, so Math.random
   is left ALONE here exactly as in the source. Static placement randomness
   lives in detail.ts / buildings.ts and is seeded there.
   ========================================================================= */

import { Container, Graphics } from "pixi.js";
import { MAT, shade } from "./iso";

const rand = (a: number, b: number): number => a + Math.random() * (b - a);

/** Common shape: every animated part exposes a node + update(t, dt). */
export interface AnimInstance {
  node: Container;
  kind: string;
  update(t: number, dt: number): void;
}

// ---- Flame / brazier -----------------------------------------------------
export class Flame implements AnimInstance {
  node: Container;
  s: number;
  glow: Graphics;
  g: Graphics;
  t: number;
  kind = "flame";

  constructor(x: number, y: number, scale?: number) {
    this.node = new Container();
    this.node.position.set(x, y);
    this.s = scale || 1;
    this.glow = new Graphics();
    this.node.addChild(this.glow);
    this.g = new Graphics();
    this.node.addChild(this.g);
    this.t = Math.random() * 10;
  }

  update(_t: number, dt: number): void {
    this.t += dt;
    const s = this.s;
    const fl = 0.9 + Math.sin(this.t * 11) * 0.14 + Math.sin(this.t * 23) * 0.06;
    const sway = Math.sin(this.t * 7) * 2.6 * s;
    const sway2 = Math.sin(this.t * 13 + 1) * 2 * s;
    const g = this.g;
    g.clear();
    // embers base
    g.ellipse(0, -4 * s, 8.5 * s, 5 * s).fill({ color: 0xb23a1e, alpha: 0.9 });
    // outer flame (taller, fuller)
    g.ellipse(sway, -15 * s * fl, 9 * s, 19 * s * fl).fill({
      color: 0xe8541f,
      alpha: 0.96,
    });
    // secondary tongue
    g.ellipse(sway2 + 3 * s, -13 * s * fl, 4.5 * s, 13 * s * fl).fill({
      color: 0xf2731f,
      alpha: 0.9,
    });
    // mid
    g.ellipse(sway * 0.7, -16 * s * fl, 5.6 * s, 14 * s * fl).fill({
      color: 0xf7a024,
      alpha: 0.97,
    });
    // core
    g.ellipse(sway * 0.5, -14 * s * fl, 3 * s, 9 * s * fl).fill({
      color: 0xffe7a0,
      alpha: 1,
    });
    g.ellipse(sway * 0.4, -11 * s * fl, 1.6 * s, 5 * s * fl).fill({
      color: 0xfff6da,
      alpha: 1,
    });
    // glow
    const gl = this.glow;
    gl.clear();
    gl.circle(0, -11 * s, 30 * s).fill({
      color: 0xf2922e,
      alpha: 0.2 + 0.07 * Math.sin(this.t * 9),
    });
  }
}

// ---- Beacon (lighthouse) — rotating light + pulsing core -----------------
export class Beacon implements AnimInstance {
  node: Container;
  s: number;
  beam: Graphics;
  core: Graphics;
  t: number;
  kind = "beacon";

  constructor(x: number, y: number, scale?: number) {
    this.node = new Container();
    this.node.position.set(x, y);
    this.s = scale || 1;
    this.beam = new Graphics();
    this.node.addChild(this.beam);
    this.core = new Graphics();
    this.node.addChild(this.core);
    this.t = Math.random() * 6;
  }

  update(_t: number, dt: number): void {
    this.t += dt;
    const s = this.s;
    const ang = this.t * 1.4;
    const b = this.beam;
    b.clear();
    // two opposed beams sweeping (in iso, widen horizontally)
    for (const dir of [0, Math.PI]) {
      const a = ang + dir;
      const dx = Math.cos(a);
      const len = 120 * s;
      const spread = 26 * s;
      const hx = dx * len;
      const hy = -Math.abs(Math.sin(a)) * 8 * s - 6 * s;
      b.poly([
        0,
        -4 * s,
        hx - spread * 0.3,
        hy - spread,
        hx + spread * 0.3,
        hy + spread,
      ]).fill({ color: 0xffe7a0, alpha: 0.16 });
    }
    const pulse = 0.7 + Math.sin(this.t * 6) * 0.3;
    const c = this.core;
    c.clear();
    c.circle(0, -5 * s, 13 * s * pulse).fill({
      color: 0xfff0c0,
      alpha: 0.3 + 0.2 * pulse,
    });
    c.circle(0, -5 * s, 5.2 * s).fill({ color: 0xffd45a, alpha: 1 });
    c.circle(0, -5 * s, 2.4 * s).fill({ color: 0xfffbec, alpha: 1 });
  }
}

// ---- Flag / banner waving ------------------------------------------------
export class Flag implements AnimInstance {
  node: Container;
  s: number;
  color: number;
  pole: Graphics;
  g: Graphics;
  t: number;
  kind = "flag";

  constructor(x: number, y: number, scale?: number, color?: number) {
    this.node = new Container();
    this.node.position.set(x, y);
    this.s = scale || 1;
    this.color = color || MAT.red;
    this.pole = new Graphics();
    this.node.addChild(this.pole);
    this.g = new Graphics();
    this.node.addChild(this.g);
    this.t = Math.random() * 8;
    this._pole();
  }

  private _pole(): void {
    const s = this.s;
    const p = this.pole;
    p.rect(-1.2 * s, -36 * s, 2.4 * s, 36 * s).fill({ color: MAT.wood });
    p.circle(0, -36 * s, 2.6 * s).fill({ color: MAT.gold });
  }

  update(_t: number, dt: number): void {
    this.t += dt;
    const s = this.s;
    const g = this.g;
    g.clear();
    const top = -34 * s;
    const h = 13 * s;
    const len = 26 * s;
    const segs = 8;
    const pts: { x: number; y: number }[] = [];
    for (let i = 0; i <= segs; i++) {
      const u = i / segs;
      const wav = Math.sin(this.t * 6 - u * 5) * 3.2 * s * u;
      pts.push({ x: u * len, y: top + wav });
    }
    const bot: { x: number; y: number }[] = [];
    for (let i = segs; i >= 0; i--) {
      const u = i / segs;
      const wav = Math.sin(this.t * 6 - u * 5) * 3.2 * s * u;
      bot.push({ x: u * len, y: top + h + wav });
    }
    const all = pts.concat(bot);
    const flat: number[] = [];
    all.forEach((p) => flat.push(p.x, p.y));
    g.poly(flat).fill({ color: this.color, alpha: 1 });
    // shaded lower third
    const flat2: number[] = [];
    pts.forEach((p) => flat2.push(p.x, p.y + h * 0.62));
    bot.forEach((p) => flat2.push(p.x, p.y));
    g.poly(flat2).fill({ color: shade(this.color, 0.8), alpha: 1 });
  }
}

// ---- Smoke — rising fading puffs ------------------------------------------
interface Puff {
  life: number;
  x: number;
  drift: number;
  r0: number;
}

export class Smoke implements AnimInstance {
  node: Container;
  s: number;
  g: Graphics;
  puffs: Puff[];
  acc: number;
  t: number;
  kind = "smoke";

  constructor(x: number, y: number, scale?: number) {
    this.node = new Container();
    this.node.position.set(x, y);
    this.s = scale || 1;
    this.g = new Graphics();
    this.node.addChild(this.g);
    this.puffs = [];
    this.acc = 0;
    this.t = 0;
  }

  update(_t: number, dt: number): void {
    this.t += dt;
    this.acc += dt;
    const s = this.s;
    if (this.acc > 0.28) {
      this.acc = 0;
      this.puffs.push({
        life: 0,
        x: rand(-2, 2) * s,
        drift: rand(-7, 10) * s,
        r0: rand(3, 5),
      });
    }
    const g = this.g;
    g.clear();
    for (let i = this.puffs.length - 1; i >= 0; i--) {
      const p = this.puffs[i];
      p.life += dt * 0.42;
      if (p.life > 1) {
        this.puffs.splice(i, 1);
        continue;
      }
      const y = -p.life * 52 * s;
      const x = p.x + p.drift * p.life;
      const r = (p.r0 + p.life * 13) * s;
      const a = 0.5 * (1 - p.life);
      g.circle(x, y, r).fill({ color: 0x7e7868, alpha: a });
      g.circle(x - r * 0.25, y - r * 0.22, r * 0.62).fill({
        color: 0x9c968a,
        alpha: a * 0.8,
      });
      g.circle(x - r * 0.4, y - r * 0.35, r * 0.34).fill({
        color: 0xb6b0a2,
        alpha: a * 0.5,
      });
    }
  }
}

// ---- Water — animated ripples over a polygon region ----------------------
export class Water implements AnimInstance {
  node: Container;
  s: number;
  pts: { x: number; y: number }[];
  mask: Graphics;
  base: Graphics;
  g: Graphics;
  b: { minx: number; maxx: number; miny: number; maxy: number };
  t: number;
  kind = "water";

  // pts: array of {x,y} screen-space polygon (the basin / harbor surface)
  constructor(pts: { x: number; y: number }[], scale?: number) {
    this.node = new Container();
    this.s = scale || 1;
    this.pts = pts;
    const flat: number[] = [];
    pts.forEach((p) => flat.push(p.x, p.y));
    this.mask = new Graphics();
    this.mask.poly(flat).fill({ color: 0xffffff });
    this.base = new Graphics();
    this.base.poly(flat).fill({ color: MAT.water });
    this.g = new Graphics();
    this.node.addChild(this.base);
    this.node.addChild(this.g);
    this.node.addChild(this.mask);
    this.g.mask = this.mask;
    // bounds
    let minx = 1e9;
    let maxx = -1e9;
    let miny = 1e9;
    let maxy = -1e9;
    pts.forEach((p) => {
      minx = Math.min(minx, p.x);
      maxx = Math.max(maxx, p.x);
      miny = Math.min(miny, p.y);
      maxy = Math.max(maxy, p.y);
    });
    this.b = { minx, maxx, miny, maxy };
    this.t = Math.random() * 5;
  }

  update(_t: number, dt: number): void {
    this.t += dt;
    const g = this.g;
    g.clear();
    const { minx, maxx, miny, maxy } = this.b;
    const s = this.s;
    const rows = Math.max(3, Math.round((maxy - miny) / (7 * s)));
    for (let r = 0; r < rows; r++) {
      const yy = miny + (r / rows) * (maxy - miny);
      const off = Math.sin(this.t * 2 + r * 0.9) * 6 * s;
      g.moveTo(minx, yy + off);
      for (let x = minx; x <= maxx; x += 10 * s) {
        g.lineTo(x, yy + off + Math.sin(this.t * 3 + x * 0.06) * 2 * s);
      }
      g.stroke({ width: 1.6 * s, color: shade(MAT.water, 1.32), alpha: 0.5 });
    }
  }
}
