// kitcd/people.ts — faithful PixiJS v8 port of the Claude Design "Polis"
// procedural citizen figures.
//
// Source: Polis-handoff/polis/project/js/people.js (PixiJS v7). This module ports
// ONLY the figure DRAWING (the `_draw` body) of the 6 citizen types. The walk /
// path-follow / oscillate movement machinery lives in AgentLayer and is NOT
// reproduced here — the source's per-frame inputs (`moving`, the walk phase, the
// hammer phase, the extinguish flag/timer) are exposed as deterministic params so
// AgentLayer can drive them from its existing 30 Hz step clock.
//
// v7 -> v8 Graphics translation applied throughout:
//   g.beginFill(c,a); g.drawPolygon([...]); g.endFill()  ->  g.poly([...]).fill({color:c,alpha:a})
//   g.beginFill(c); g.drawRect(x,y,w,h); g.endFill()      ->  g.rect(x,y,w,h).fill({color:c})
//   g.beginFill(c); g.drawCircle(x,y,r); g.endFill()      ->  g.circle(x,y,r).fill({color:c})
//   g.beginFill(c,a); g.drawEllipse(x,y,rx,ry); g.endFill() -> g.ellipse(x,y,rx,ry).fill({color:c,alpha:a})
//   g.lineStyle({width,color,...}); g.moveTo; g.lineTo; g.lineStyle(0)
//                                                        ->  g.moveTo().lineTo().stroke({width,color,alpha,cap})
// The source's `limb(g,ax,ay,bx,by,w,col)` helper (a round-capped line) becomes
// `limb()` below using a single .moveTo().lineTo().stroke({cap:'round'}).
//
// Coordinate space: the source authors figures with feet/shadow at y=0 and the
// head at NEGATIVE y (head circle ~ -19*s). The host container is screen-space
// (y-down), so negative-y reads as "up" on screen — exactly how AgentLayer's old
// omino was drawn (head at negative y, legs at positive y). The geometry is
// therefore reproduced VERBATIM with no extra y-flip.
//
// Colors are the source's `S(hex, factor) = ISO.shade(hex, factor)`. We reproduce
// `shade` here (multiply each RGB channel by the factor, clamped) so every derived
// tone matches the original 1:1.
//
// Determinism: drawing is a pure function of (type, opts). No Math.random — the
// per-citizen tunic and the walk/action phases are all supplied by the caller
// (derived from the agentId + step frame in AgentLayer). `t` in the source seeded
// the firefighter water-throw cadence and the builder hammer; we expose those as
// `actionPhase`.

import type { Graphics } from "pixi.js";

// ---------------------------------------------------------------------------
// Constants (verbatim from people.js)
// ---------------------------------------------------------------------------
const SK = 0xcb9f6e; // skin (lit)
const SKd = 0xa67c4c; // skin (shadow)
const HAIR = 0x35261a;

// Per-type default tunic colours (verbatim TUNIC map from the source).
const TUNIC: Record<CitizenType, number> = {
  citizen: 0xe6dcc4,
  builder: 0x8a6234,
  firefighter: 0xb23a30,
  watercarrier: 0xbfc7cc,
  merchant: 0xc98a2b,
  noble: 0xf3edde,
  priest: 0xf0ece0,  // long white robe base
  foreigner: 0x7a9e9a, // muted teal travel cloak
};

/** The source's default tunic colour for a figure type (read-only lookup). */
export function defaultTunic(type: CitizenType): number {
  return TUNIC[type] ?? 0xe6dcc4;
}

/** shade(hex, factor): multiply each RGB channel by `factor`, clamped — the same
 *  tone transform the figures use internally. Exposed so callers can derive a
 *  subtly-varied per-citizen tunic that stays on-palette. */
export function shadeColor(hex: number, factor: number): number {
  return shade(hex, factor);
}

export type CitizenType =
  | "citizen"
  | "builder"
  | "firefighter"
  | "watercarrier"
  | "merchant"
  | "noble"
  | "priest"
  | "foreigner";

export interface CitizenDrawOpts {
  /** true while the agent is moving (drives leg swing + arm sway via Math.sin). */
  moving: boolean;
  /** walk-cycle phase in radians (the source's `this.walkPhase`, advanced while
   *  moving). Math.sin(phase) is the stride amplitude. */
  phase: number;
  /**
   * Action phase in radians, role-specific (the source's per-update inputs):
   *  - builder      : hammer-swing phase (source used `this.t * 5`); the arm
   *                   angle is `-1.2 + Math.sin(actionPhase) * 1.0` and sparks
   *                   fly when sin(actionPhase) > 0.86.
   *  - firefighter  : water-throw timer (source used `this.t`); the arc shows
   *                   while `(actionPhase * 0.9) % 1 < 0.45`. Pass 0 to never
   *                   throw water (idle bucket only).
   * Other types ignore it.
   */
  actionPhase: number;
  /** explicit tunic override; if omitted, the per-type default TUNIC is used. */
  tunic?: number;
  /** When set, draws a carried wooden crate in front of the figure at hand height.
   *  The crate bobs with the walk phase for a natural carry feel. */
  carrying?: "crate";
}

// shade(hex, factor): multiply each RGB channel by `factor`, clamp to [0,255].
// Reproduces ISO.shade from the source so derived tones match exactly.
function shade(hex: number, f: number): number {
  let r = (hex >> 16) & 0xff;
  let g = (hex >> 8) & 0xff;
  let b = hex & 0xff;
  r = Math.max(0, Math.min(255, Math.round(r * f)));
  g = Math.max(0, Math.min(255, Math.round(g * f)));
  b = Math.max(0, Math.min(255, Math.round(b * f)));
  return (r << 16) | (g << 8) | b;
}

// limb(): a single round-capped line (the source's `limb` helper).
function limb(
  g: Graphics,
  ax: number,
  ay: number,
  bx: number,
  by: number,
  w: number,
  col: number,
): void {
  g.moveTo(ax, ay)
    .lineTo(bx, by)
    .stroke({ width: w, color: col, cap: "round" });
}

// ---------------------------------------------------------------------------
// Public entry: draw ONE frame of a citizen into `g`. Clears + redraws `g`,
// exactly like the source `_draw()` (limbs animate by clear+redraw). The caller
// (AgentLayer) gates this to LOD-visible agents only.
// ---------------------------------------------------------------------------
export function drawCitizen(
  g: Graphics,
  type: CitizenType,
  opts: CitizenDrawOpts,
): void {
  const s = 1; // unit scale; AgentLayer scales the whole node via zoom.
  const { moving, phase } = opts;
  const tunic = opts.tunic ?? TUNIC[type] ?? 0xe6dcc4;
  const tDk = shade(tunic, 0.78);

  // The source seeds the builder hammer from `this.t * 5` and leaves it 0 for
  // everyone else; firefighter water-throw is timed off `this.t`. We carry both
  // through `actionPhase`.
  const hammerPhase = type === "builder" ? opts.actionPhase : 0;
  const extinguish = type === "firefighter" ? opts.actionPhase : 0;

  g.clear();

  const sw = moving ? Math.sin(phase) : 0;
  const hipY = -7.5 * s;
  const shY = -15 * s;

  // ---- shadow ----
  g.ellipse(0, 0, 6 * s, 2.2 * s).fill({ color: 0x241a10, alpha: 0.16 });

  // ---- legs (front opposite to back) ----
  limb(g, 0.4 * s, hipY, sw * 2.6 * s, 0, 2.3 * s, SKd);
  limb(g, -0.4 * s, hipY, -sw * 2.6 * s, 0, 2.5 * s, SK);
  // feet
  g.ellipse(sw * 2.6 * s, 0, 2 * s, 1 * s).fill({ color: 0x4a3320 });
  g.ellipse(-sw * 2.6 * s, 0, 2 * s, 1 * s).fill({ color: 0x4a3320 });

  // ---- tunic ----
  g.poly([
    -3.6 * s, hipY + 0.6 * s,
    3.6 * s, hipY + 0.6 * s,
    2.6 * s, -16.4 * s,
    -2.6 * s, -16.4 * s,
  ]).fill({ color: tunic });
  // tunic shadow (right half)
  g.poly([
    0.3 * s, hipY + 0.6 * s,
    3.6 * s, hipY + 0.6 * s,
    2.6 * s, -16.4 * s,
    0.3 * s, -16.4 * s,
  ]).fill({ color: tDk, alpha: 0.5 });
  // belt line (gold for noble, else dark tunic)
  g.moveTo(-3 * s, -9.2 * s)
    .lineTo(3 * s, -9.2 * s)
    .stroke({ width: 1.1 * s, color: type === "noble" ? 0xc9a03a : tDk });

  // ---- back accessory (behind body): merchant sack ----
  if (type === "merchant") {
    g.ellipse(-2.8 * s, -13.5 * s, 3 * s, 3.8 * s).fill({ color: 0xb89a5c });
    g.ellipse(-3.4 * s, -12.4 * s, 1.3 * s, 2.4 * s).fill({
      color: shade(0xb89a5c, 0.82),
    });
  }

  // ---- arms ----
  if (type === "builder") {
    // back arm
    limb(g, -2.7 * s, shY, -3.3 * s, -8.6 * s, 2 * s, SKd);
    // swinging front arm + hammer
    const a = -1.2 + Math.sin(hammerPhase) * 1.0;
    const sx = 2.7 * s;
    const sy = shY;
    const ex = sx + Math.cos(a) * 7 * s;
    const ey = sy + Math.sin(a) * 7 * s;
    limb(g, sx, sy, ex, ey, 2 * s, SK);
    // hammer handle
    const hx = ex + Math.cos(a) * 4 * s;
    const hy = ey + Math.sin(a) * 4 * s;
    g.moveTo(ex, ey)
      .lineTo(hx, hy)
      .stroke({ width: 1.5 * s, color: 0x6e4a2a, cap: "round" });
    // hammer head + highlight
    g.rect(hx - 2.3 * s, hy - 2 * s, 4.6 * s, 3.1 * s).fill({ color: 0x55555e });
    g.rect(hx - 2.3 * s, hy - 2 * s, 4.6 * s, 1 * s).fill({
      color: shade(0x55555e, 1.25),
    });
    // impact sparks at the bottom of the swing
    if (Math.sin(hammerPhase) > 0.86) {
      for (let k = 0; k < 4; k++) {
        const ka = k * 1.6;
        g.circle(
          hx + Math.cos(ka) * 3 * s,
          hy + 2.4 * s + Math.sin(ka) * 2 * s,
          0.9 * s,
        ).fill({ color: 0xffe6a0, alpha: 0.9 });
      }
    }
  } else {
    const aSw = sw * 2.4 * s;
    // back arm swings with the walk
    limb(g, -2.7 * s, shY, -3.3 * s - aSw, -8.6 * s, 2 * s, SKd);
    if (type === "firefighter") {
      // front arm holds a bucket
      limb(g, 2.7 * s, shY, 3.7 * s, -10 * s, 2 * s, SK);
      const bx = 4.4 * s;
      const by = -8.6 * s;
      g.rect(bx - 2.1 * s, by - 3 * s, 4.2 * s, 4.2 * s).fill({ color: 0x6e4a2a });
      g.rect(bx - 1.7 * s, by - 3 * s, 3.4 * s, 1.5 * s).fill({
        color: shade(0x3c7b92, 1.15),
      });
      g.moveTo(bx - 2.1 * s, by - 3 * s)
        .lineTo(bx + 2.1 * s, by - 3 * s)
        .stroke({ width: 0.9 * s, color: 0x4a3320 });
    } else {
      // ordinary front arm swings with the walk
      limb(g, 2.7 * s, shY, 3.3 * s + aSw, -8.6 * s, 2 * s, SK);
    }
  }

  // ---- water-carrier yoke + amphorae (over shoulders) ----
  if (type === "watercarrier") {
    g.moveTo(-6.2 * s, shY - 0.5 * s)
      .lineTo(6.2 * s, shY - 0.5 * s)
      .stroke({ width: 1.5 * s, color: 0x6e4a2a, cap: "round" });
    for (const x of [-6, 6]) {
      // rope
      g.moveTo(x * s, shY - 0.3 * s)
        .lineTo(x * s, -11 * s)
        .stroke({ width: 0.8 * s, color: 0x4a3320 });
      // amphora body + highlight
      g.ellipse(x * s, -9 * s, 2.4 * s, 3.6 * s).fill({
        color: shade(0xc0613a, 0.95),
      });
      g.ellipse(x * s - 0.8 * s, -9.8 * s, 0.9 * s, 2 * s).fill({
        color: shade(0xc0613a, 1.15),
      });
    }
  }

  // ---- head ----
  g.circle(0, -19.3 * s, 2.95 * s).fill({ color: HAIR });
  g.circle(0, -18.3 * s, 2.6 * s).fill({ color: SK });

  // ---- noble himation cloak + staff ----
  if (type === "noble") {
    g.poly([
      -4 * s, hipY,
      1.4 * s, hipY,
      2.4 * s, -14.5 * s,
      -1.6 * s, -16 * s,
      -4.6 * s, -11 * s,
    ]).fill({ color: 0xefe7d2 });
    g.moveTo(-4.6 * s, -11 * s)
      .lineTo(-4 * s, hipY)
      .stroke({ width: 1 * s, color: 0x7a3f86, alpha: 0.85 });
    // staff
    g.moveTo(3.6 * s, -17 * s)
      .lineTo(3.6 * s, 0)
      .stroke({ width: 1.1 * s, color: 0x6e4a2a });
  }

  // ---- priest: long white robe, purple trim band, laurel circlet ----
  if (type === "priest") {
    // purple trim band across the tunic at chest height
    g.moveTo(-3.4 * s, -12.8 * s)
      .lineTo(3.4 * s, -12.8 * s)
      .stroke({ width: 1.3 * s, color: 0x7a3f86 });
    g.moveTo(-3.4 * s, -12.2 * s)
      .lineTo(3.4 * s, -12.2 * s)
      .stroke({ width: 0.8 * s, color: 0x7a3f86, alpha: 0.5 });
    // laurel circlet: small leaf-shaped ovals around the crown
    for (let i = 0; i < 5; i++) {
      const a = -Math.PI * 0.8 + (i * Math.PI * 0.4) / 4;
      const lx = Math.cos(a) * 3.2 * s;
      const ly = -19 * s + Math.sin(a) * 3.2 * s;
      g.ellipse(lx, ly, 1.2 * s, 0.5 * s).fill({ color: 0x6a8a3e });
    }
  }

  // ---- foreigner: hooded teal cloak + walking staff ----
  if (type === "foreigner") {
    // hood over the head
    g.poly([
      -3.5 * s, -19.5 * s,
      0, -23.5 * s,
      3.5 * s, -19.5 * s,
    ]).fill({ color: 0x6a8e8a });
    // cloak drape over shoulders
    g.poly([
      -4.2 * s, hipY,
      4.2 * s, hipY,
      3.6 * s, -15 * s,
      -3.6 * s, -15 * s,
    ]).fill({ color: shade(0x7a9e9a, 0.88) });
    // walking staff in the right hand
    g.moveTo(3.8 * s, -16 * s)
      .lineTo(3.8 * s, 0)
      .stroke({ width: 1.2 * s, color: 0x5a4020, cap: "round" });
  }

  // ---- carried crate (trade porters) ----
  if (opts.carrying === "crate") {
    // Vertical bob synced to walk phase: reuses the same Math.sin the walk
    // cycle uses, so the crate bobs in step with the legs.
    const bob = moving ? Math.sin(phase) * 0.8 * s : 0;
    const cx = 0; // centered in front of the figure
    const cy = -8 * s + bob; // hand height
    const cw = 4 * s;
    const ch = 3 * s;
    // crate body
    g.rect(cx - cw / 2, cy - ch / 2, cw, ch).fill({ color: 0x8a6a3a });
    g.rect(cx - cw / 2, cy - ch / 2, cw, ch).stroke({ width: 0.6 * s, color: 0x5a4a28 });
    // rope cross
    g.moveTo(cx - cw / 2, cy).lineTo(cx + cw / 2, cy)
      .stroke({ width: 0.5 * s, color: 0x4a3a1a });
    g.moveTo(cx, cy - ch / 2).lineTo(cx, cy + ch / 2)
      .stroke({ width: 0.5 * s, color: 0x4a3a1a });
  }

  // ---- firefighter water-throw arc ----
  if (extinguish) {
    const ph = (extinguish * 0.9) % 1;
    if (ph < 0.45) {
      for (let k = 0; k < 6; k++) {
        const tt = k / 5;
        const px = (5 + tt * 13) * s;
        const py = -11 * s - Math.sin(tt * Math.PI) * 9 * s;
        g.circle(px, py, (1.4 - tt * 0.5) * s).fill({
          color: 0x7cc0da,
          alpha: 0.85,
        });
      }
    }
  }
}
