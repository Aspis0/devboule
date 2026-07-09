// growthEffects.ts — Polis L2 "real agents grow buildings" visual layer.
//
// DATA-DRIVEN, FRONTEND ONLY. Every effect here is triggered by a signal that
// ALREADY exists on the wire (no backend / Rust / MCP change):
//   - SCAFFOLDING  ← Building.agentPresent is SET (a REAL agent is working here).
//   - TIER-GROWTH  ← Building.visualTier INCREASED across a live diff.
//   - POP-IN       ← a building was ADDED across a live diff.
//   - RUBBLE       ← a building was REMOVED across a live diff.
//   - GOLDEN-SEAL  ← Building.agentPresent transitioned SET → UNSET (work done).
//
// MOCK/REAL BOUNDARY: these are for REAL data only. The decorative ambient crowd
// (AmbientLayer / L1) never triggers any of them — scaffolding keys on the real
// `agentPresent` signal, growth/pop/rubble/seal on real add/remove/tier deltas.
//
// PERFORMANCE CONTRACT (matches effects.ts / the kit anims discipline):
//   - Scaffolding is an AnimInstance (node + update) parented INTO the building
//     node's container, so it is animated ONLY for visible chunks (the renderer
//     ticks node.kitAnims for visible chunks only) and DESTROYED WITH THE NODE
//     (container.destroy({children:true})) — no removeFromParent leak.
//   - One-shot bursts (pop-in / rubble / tier-dust / seal) come from a FIXED
//     POOL on a dedicated effects container; an idle slot is parked invisible and
//     reused. Alloc-free in steady state (no per-frame `new`). When the pool is
//     full the oldest active slot is recycled (a burst is never critical).
//   - One-shot redraw is skipped when the burst's world point is outside the
//     visible bounds (LOD/cull-gated like the rest of the renderer); the short
//     timer still advances so the slot self-recycles. Torn down in dispose().
//   - The tier-grow / pop-in node TRANSFORM transitions mutate scale/alpha on the
//     freshly-built node container only (no geometry rebuild) and run AFTER the
//     diff's node mutation, keyed on deltas computed at diff time — they never
//     touch the chunked-build / diff / cull or the F0 idle|building|diffing state
//     machine.

import { Container, Graphics, Rectangle, Text, TextStyle } from "pixi.js";
import { PALETTE, DERIVED } from "./palette";
import { darken, lighten } from "./iso";
import { Flame, Smoke, type AnimInstance } from "./kitcd/anims";
import type { SinSeverity } from "../../types/city";

// Derived effect tones — stay on-palette (every color is a pure function of a
// PALETTE entry, per the COLOR CONTRACT in palette.ts).
const TIMBER = darken(PALETTE.stoneDark, 0.12); // scaffolding poles
const TIMBER_LIT = lighten(PALETTE.sandDark, 0.04); // sunlit lashing/planks
const SHIMMER = lighten(PALETTE.goldAccent, 0.18); // faint "build" energy
const DUST = lighten(PALETTE.sandDark, 0.1); // grow/pop dust
const RUBBLE_STONE = PALETTE.stoneDark; // collapsed debris
const SEAL_GOLD = PALETTE.goldAccent; // completion celebration
const SEAL_GOLD_HOT = lighten(PALETTE.goldAccent, 0.26);

// ---------------------------------------------------------------------------
// SCAFFOLDING — persistent overlay while an agent works (keyed on agentPresent).
// ---------------------------------------------------------------------------
//
// An AnimInstance: the renderer pushes it into node.kitAnims so it animates only
// for visible chunks and is destroyed with the node container. Static timber
// posts (drawn ONCE) + a faint stepped "build shimmer" band that rises and
// fades on a loop (the only per-frame redraw, a tiny Graphics).
export class Scaffold {
  node: Container;
  kind = "scaffold";
  private shimmer: Graphics;
  private hw: number;
  private depth: number;
  private t: number;

  /** @param hw    footprint half-width in px (from BuiltBuilding.hw).
   *  @param depth silhouette height in px above the anchor (BuiltBuilding.depth). */
  constructor(hw: number, depth: number) {
    this.node = new Container();
    this.hw = Math.max(8, hw);
    this.depth = Math.max(12, depth);
    this.t = 0;

    // ---- static timber posts (drawn once) ----
    const posts = new Graphics();
    // Two front corner posts framing the footprint, plus a top rail and a couple
    // of putlogs (cross-pieces) — reads as builder's scaffolding at a glance.
    const x = this.hw * 0.92;
    const postTop = -this.depth * 0.96;
    const lashY1 = postTop * 0.32;
    const lashY2 = postTop * 0.7;
    // left + right vertical poles
    for (const sx of [-x, x]) {
      posts
        .moveTo(sx, 2)
        .lineTo(sx, postTop)
        .stroke({ width: 2.4, color: TIMBER, alpha: 0.92 });
    }
    // top rail + two putlogs (horizontals)
    for (const ry of [postTop, lashY2, lashY1]) {
      posts
        .moveTo(-x, ry)
        .lineTo(x, ry)
        .stroke({ width: 2, color: TIMBER, alpha: 0.85 });
    }
    // a diagonal brace + lashing knobs for the wooden-rig read
    posts
      .moveTo(-x, 2)
      .lineTo(x, lashY1)
      .stroke({ width: 1.4, color: TIMBER_LIT, alpha: 0.6 });
    for (const sx of [-x, x]) {
      for (const ry of [postTop, lashY2, lashY1]) {
        posts.circle(sx, ry, 1.6).fill({ color: TIMBER_LIT, alpha: 0.7 });
      }
    }
    this.node.addChild(posts);

    this.shimmer = new Graphics();
    this.node.addChild(this.shimmer);
  }

  update(_t: number, dt: number): void {
    this.t += dt;
    const g = this.shimmer;
    g.clear();
    // A faint "work energy" band that rises up the scaffold on a ~1.4s loop and
    // fades at the top — the cue that the building is actively being grown. One
    // small redraw per visible step (the kit anims do the same).
    const loop = (this.t % 1.4) / 1.4;
    const y = 2 + (-this.depth * 0.96 - 2) * loop; // bottom → top
    const a = 0.32 * (1 - loop); // brightest at the base, fades rising
    const w = this.hw * 0.92;
    g.moveTo(-w, y).lineTo(w, y).stroke({ width: 2.2, color: SHIMMER, alpha: a });
    // a couple of rising sparks for a livelier rig
    g.circle(-w * 0.4, y - 1, 1.4).fill({ color: SHIMMER, alpha: a * 0.9 });
    g.circle(w * 0.45, y + 1, 1.2).fill({ color: SHIMMER, alpha: a * 0.7 });
  }
}

// ---------------------------------------------------------------------------
// DISASTER — persistent on-map fire/smoke overlay for a building with URBAN SINS
// (keyed on the WORST sin severity). Frontend-only, DATA-DRIVEN: it exists iff
// `Building.sins` is non-empty (a real, backend-detected problem) and AUTO-CLEARS
// because `diffCity.buildingChanged` rebuilds the node on a worst-severity change
// — when the file is fixed its sins clear, the node is rebuilt WITHOUT this
// overlay, and the fire/smoke disappears. No separate clearing mechanism.
// ---------------------------------------------------------------------------
//
// Mirrors `Scaffold`: an `AnimInstance` parented INTO the building node's
// container (so it pans/culls/destroys with the node — no removeFromParent leak)
// and pushed into `node.kitAnims` so the renderer's visible-chunk-only step clock
// drives it for free. It does NOT author any new fire/smoke geometry — it COMPOSES
// the Claude-Design kit `Flame`/`Smoke` parts (scaled to the footprint, tinted for
// inferno), exactly the art used for the ambient building decorations.
//
// Inferno tint: the kit `Flame` is orange; the inferno pole multiplies it toward
// an angry red via the container `tint` (a derived PALETTE.terracotta tone — stays
// on-palette, no fresh hex). Smoke is left untinted (its gray reads correctly).
const INFERNO_TINT = lighten(PALETTE.terracotta, 0.32); // reddish multiply for the flame

export class Disaster implements AnimInstance {
  node: Container;
  kind = "disaster";
  /** The kit anims COMPOSED into this overlay (Flame/Smoke). Driven by the
   *  Disaster's own `update`, which the renderer ticks via `node.kitAnims`. */
  private parts: AnimInstance[];

  /**
   * @param severity worst sin severity ("smoke" | "fire" | "inferno"), from
   *                 `diffCity.worstSinSeverity` (the single source of truth).
   * @param hw       footprint half-width in px (BuiltBuilding.hw) — sizes/places
   *                 the flame + smoke columns over the building footprint.
   * @param depth    silhouette height in px above the anchor (BuiltBuilding.depth)
   *                 — the smoke/flame sit at roughly roof height.
   */
  constructor(severity: SinSeverity, hw: number, depth: number) {
    this.node = new Container();
    const w = Math.max(8, hw);
    const d = Math.max(12, depth);
    // Roof / upper-body anchor: the fire eats the building from the roofline up.
    const roofY = -d * 0.62;
    this.parts = [];

    // Per-severity composition. SMOKE: a couple of gray wisps, no flame. FIRE: an
    // orange flame + a light smoke column. INFERNO: a bigger flame, RED-tinted,
    // plus more/heavier smoke. Scales are derived from the footprint so a big
    // building burns bigger than a hut. All parts are kit Flame/Smoke instances.
    if (severity === "smoke") {
      const sc = 0.22 + w * 0.004;
      this.add(new Smoke(-w * 0.18, roofY, sc));
      this.add(new Smoke(w * 0.22, roofY - d * 0.08, sc * 0.85));
    } else if (severity === "fire") {
      const fl = 0.4 + w * 0.006;
      this.add(new Flame(0, roofY, fl));
      this.add(new Smoke(w * 0.12, roofY - d * 0.05, 0.28 + w * 0.004));
    } else {
      // inferno — engulfed: a tall central flame + two flanking tongues, RED
      // tinted, with heavier smoke/embers above.
      const fl = 0.6 + w * 0.009;
      const center = new Flame(0, roofY, fl);
      const left = new Flame(-w * 0.42, roofY + d * 0.06, fl * 0.62);
      const right = new Flame(w * 0.42, roofY + d * 0.06, fl * 0.62);
      // Push the kit's orange toward red (multiply tint on each flame container).
      center.node.tint = INFERNO_TINT;
      left.node.tint = INFERNO_TINT;
      right.node.tint = INFERNO_TINT;
      this.add(center);
      this.add(left);
      this.add(right);
      this.add(new Smoke(-w * 0.1, roofY - d * 0.12, 0.4 + w * 0.006));
      this.add(new Smoke(w * 0.2, roofY - d * 0.18, 0.34 + w * 0.005));
    }
  }

  private add(part: AnimInstance): void {
    this.node.addChild(part.node);
    this.parts.push(part);
  }

  /**
   * P5.1 — suppress legacy Flame/Smoke rendering when the new crowd-fire
   * flip-book system (Tier F1) is active. The crowd-fire Sprites on the effects
   * layer replace the per-frame clear+redraw entirely.
   */
  private legacyVisible = true;

  /** Toggle legacy Flame/Smoke visibility. Called by PolisRenderer when crowd
   *  fires are created/removed for this building. */
  setLegacyVisible(visible: boolean): void {
    this.legacyVisible = visible;
    for (const p of this.parts) p.node.visible = visible;
  }

  /** Drive every composed kit part. Called by the renderer's step clock only for
   *  VISIBLE chunks (and skipped entirely when the LOD pass has hidden `node`),
   *  so the per-frame clear+redraw cost is bounded to on-screen, zoomed-in fires.
   *  No per-frame allocation beyond the kit parts' inherent clear/redraw. */
  update(t: number, dt: number): void {
    // LOD gate: when the renderer hides this overlay (far zoom) skip the redraw
    // entirely — the kit parts each clear()+refill a Graphics, so this keeps a
    // zoomed-out city's fires from costing anything per step.
    if (!this.node.visible) return;
    // P5.1 — if legacy rendering is suppressed (crowd fires active), skip.
    if (!this.legacyVisible) return;
    const parts = this.parts;
    for (let i = 0; i < parts.length; i++) parts[i].update(t, dt);
  }
}

// ---------------------------------------------------------------------------
// INVESTIGATION — persistent "under investigation" overlay (bug-investigation P3).
// Keyed on `Building.suspectOfCardId`: a building that an OPEN bug card's Oracle
// suspect files resolved to. Frontend-only, DATA-DRIVEN, and AUTO-CLEARS exactly
// like `Disaster`: `diffCity.buildingChanged` rebuilds the node when
// `suspectOfCardId` appears/clears, so the smoke shows the moment the bug card
// exists and vanishes when the card is resolved (status "done") or deleted.
// ---------------------------------------------------------------------------
//
// Mirrors `Disaster` structurally: an `AnimInstance` parented INTO the building
// node container (pans/culls/destroys with the node — no removeFromParent leak)
// and pushed into `node.kitAnims` so the renderer's visible-chunk-only step clock
// drives it. It authors NO new fire/smoke geometry — it COMPOSES the kit `Smoke`
// part, TINTED indigo/violet (`DERIVED.investigate`, on-palette) + a small "?"
// `Text` marker above it.
//
// HONESTY (non-negotiable): a suspect is Oracle's GUESS, so the overlay must read
// as DIFFERENT from a confirmed disaster — DIFFERENT COLOR FAMILY (blue/violet,
// never the orange/red fire) and a DIFFERENT MARKER ("?", a question, not flames).
// It COEXISTS with a `Disaster` on the same building (both are independent children
// of the node container, neither clobbers the other) so a confirmed-disaster file
// that is also a bug suspect honestly shows BOTH the fire and the question smoke.
const INVESTIGATION_TINT = DERIVED.investigate; // indigo/violet smoke multiply

export class Investigation implements AnimInstance {
  node: Container;
  kind = "investigation";
  /** The kit `Smoke` parts composed into this overlay, tinted investigative blue.
   *  Driven by this instance's `update` via the renderer's `node.kitAnims`. */
  private parts: AnimInstance[];
  /** The "?" marker — a static `Text` drawn once (no per-frame work). */
  private mark: Text;

  /**
   * @param hw    footprint half-width in px (BuiltBuilding.hw) — sizes/places the
   *              smoke columns + the "?" marker over the building footprint.
   * @param depth silhouette height in px above the anchor (BuiltBuilding.depth) —
   *              the smoke + marker sit at roughly roof height.
   */
  constructor(hw: number, depth: number) {
    this.node = new Container();
    const w = Math.max(8, hw);
    const d = Math.max(12, depth);
    // Roof / upper-body anchor: the question smoke rises off the roofline.
    const roofY = -d * 0.62;
    this.parts = [];

    // A couple of tinted smoke wisps — deliberately LIGHTER than a disaster's
    // smoke column (a question, not a fire). Scale derives from the footprint so a
    // big building's smoke reads bigger than a hut's. Tint each kit Smoke container
    // toward the investigative indigo (multiply on `.node.tint`).
    const sc = 0.2 + w * 0.0035;
    const a = new Smoke(-w * 0.16, roofY, sc);
    const b = new Smoke(w * 0.2, roofY - d * 0.07, sc * 0.82);
    a.node.tint = INVESTIGATION_TINT;
    b.node.tint = INVESTIGATION_TINT;
    this.add(a);
    this.add(b);

    // "?" marker — the HONEST cue that this is a SUSPECT (a question), distinct
    // from a disaster's flames. A static Text drawn once, anchored above the smoke
    // at roof height. Style mirrors the renderer's small in-world Text usage (Inter
    // family, cream stroke for legibility) but coloured with the investigative
    // shade so the marker matches the smoke family.
    this.mark = new Text({
      text: "?",
      style: new TextStyle({
        fontFamily: "Inter, system-ui, sans-serif",
        fontSize: 13,
        fontWeight: "700",
        fill: DERIVED.investigateMark,
        stroke: { color: PALETTE.cream, width: 3 },
        align: "center",
      }),
    });
    this.mark.anchor.set(0.5, 1);
    // Above the smoke wisps, near the top of the silhouette.
    this.mark.position.set(0, roofY - d * 0.14);
    this.node.addChild(this.mark);
  }

  private add(part: AnimInstance): void {
    this.node.addChild(part.node);
    this.parts.push(part);
  }

  /** Drive the composed kit smoke. LOD-gated like `Disaster`: when the renderer
   *  hides this overlay (far zoom) the per-step redraw is skipped entirely. The
   *  "?" marker is static (drawn once) so it costs nothing per step. No per-frame
   *  allocation beyond the kit Smoke's inherent clear/redraw. */
  update(t: number, dt: number): void {
    if (!this.node.visible) return;
    const parts = this.parts;
    for (let i = 0; i < parts.length; i++) parts[i].update(t, dt);
  }
}

// ---------------------------------------------------------------------------
// ONE-SHOT BURSTS — pooled transient effects on a dedicated effects container.
// ---------------------------------------------------------------------------

type FxKind = "dust" | "rubble" | "seal";

// A single recycled particle inside a burst slot. Pre-allocated; reset per use.
interface FxParticle {
  active: boolean;
  x: number;
  y: number;
  vx: number;
  vy: number;
  r0: number;
  life: number; // 0..1
  rate: number; // life/sec
  color: number;
  alpha0: number;
  ring: boolean; // a seal expands as a ring instead of drifting
}

const PARTICLES_PER_SLOT = 12;

// One reusable burst slot: its own Graphics on the effects layer, a fixed
// particle array, parked invisible when idle. No per-frame allocation.
class FxSlot {
  g: Graphics;
  active = false;
  x = 0;
  y = 0;
  // Monotonic arm sequence — when the pool is saturated we evict the slot with
  // the smallest armSeq (the OLDEST-armed burst), not the oldest-created slot,
  // so a freshly re-armed slot can't be stomped while a stale one survives.
  // 0 = never armed (always evicted first).
  armSeq = 0;
  private parts: FxParticle[] = [];

  constructor() {
    this.g = new Graphics();
    this.g.visible = false;
    this.g.eventMode = "none";
    for (let i = 0; i < PARTICLES_PER_SLOT; i++) {
      this.parts.push({
        active: false,
        x: 0,
        y: 0,
        vx: 0,
        vy: 0,
        r0: 0,
        life: 0,
        rate: 0,
        color: 0,
        alpha0: 0,
        ring: false,
      });
    }
  }

  /** Arm this slot at world (x,y) with a burst preset. Reuses the particle
   *  array in place — no allocation. */
  arm(kind: FxKind, x: number, y: number): void {
    this.active = true;
    this.armSeq = ++armCounter;
    this.x = x;
    this.y = y;
    this.g.position.set(x, y);
    this.g.visible = true;
    this.g.alpha = 1;
    const n = this.parts.length;
    for (let i = 0; i < n; i++) {
      const p = this.parts[i];
      // Deterministic-enough spread from the index (no Math.random needed; the
      // burst is decorative + one-shot, so a fixed fan reads fine and stays
      // alloc/seed-free).
      const ang = (i / n) * Math.PI * 2 + (kind === "rubble" ? 0.4 : 0);
      p.active = true;
      p.life = 0;
      p.ring = false;
      if (kind === "dust") {
        // soft beige puff drifting up + out (tier-grow / pop-in).
        p.x = Math.cos(ang) * 3;
        p.y = -2 + Math.sin(ang) * 1.5;
        p.vx = Math.cos(ang) * 16;
        p.vy = -18 - (i % 3) * 5;
        p.r0 = 3 + (i % 4);
        p.rate = 1.6;
        p.color = DUST;
        p.alpha0 = 0.5;
      } else if (kind === "rubble") {
        // heavier debris that falls + settles (removed building).
        p.x = Math.cos(ang) * 4;
        p.y = -4;
        p.vx = Math.cos(ang) * 22;
        p.vy = -8 - (i % 4) * 4; // pops up then gravity pulls down
        p.r0 = 2 + (i % 3);
        p.rate = 1.3;
        p.color = i % 2 ? RUBBLE_STONE : DUST;
        p.alpha0 = 0.7;
      } else {
        // GOLDEN SEAL — an expanding gold ring + a few rising motes (Augur seal).
        p.ring = i === 0; // first particle is the expanding ring
        p.x = Math.cos(ang) * 2;
        p.y = -6 + Math.sin(ang) * 2;
        p.vx = Math.cos(ang) * 10;
        p.vy = -26 - (i % 3) * 6;
        p.r0 = i === 0 ? 6 : 1.6 + (i % 3);
        p.rate = i === 0 ? 1.1 : 1.5;
        p.color = i % 2 ? SEAL_GOLD : SEAL_GOLD_HOT;
        p.alpha0 = i === 0 ? 0.9 : 0.95;
      }
    }
  }

  /** Advance the burst by dt seconds. `draw` redraws the Graphics (skipped when
   *  off-screen). Returns true while any particle is still alive. */
  step(dt: number, draw: boolean): boolean {
    if (!this.active) return false;
    const g = this.g;
    if (draw) g.clear();
    let alive = false;
    for (const p of this.parts) {
      if (!p.active) continue;
      p.life += dt * p.rate;
      if (p.life >= 1) {
        p.active = false;
        continue;
      }
      alive = true;
      if (!draw) continue;
      // integrate (gravity for the falling debris; smooth drift otherwise)
      const t = p.life;
      const x = p.x + p.vx * t;
      // rubble arcs (gravity); dust/seal rise + ease out
      const y = p.ring ? p.y : p.y + p.vy * t + (p.color === RUBBLE_STONE ? 60 * t * t : 0);
      const a = p.alpha0 * (1 - t);
      if (p.ring) {
        // expanding celebratory ring
        const rr = p.r0 + t * 26;
        g.circle(0, p.y, rr).stroke({ width: 2, color: p.color, alpha: a });
      } else {
        const r = Math.max(0.5, p.r0 * (1 - t * 0.5));
        g.circle(x, y, r).fill({ color: p.color, alpha: a });
      }
    }
    if (!alive) {
      this.active = false;
      this.g.visible = false;
    }
    return alive;
  }

  /** Park invisible without finishing (used on a full-pool recycle). */
  retire(): void {
    this.active = false;
    this.g.visible = false;
    for (const p of this.parts) p.active = false;
  }
}

const POOL_SIZE = 16;

// Monotonic arm tick, shared across all slots. Stamped on each arm() so the
// saturated-pool path can evict the oldest-ARMED slot (min armSeq). Module-level
// is fine: it only needs to be monotonic within a session, and a u53 won't wrap
// in any realistic run.
let armCounter = 0;

/**
 * GrowthFx — owns the one-shot burst pool on a dedicated effects container and
 * (separately) the node TRANSFORM transitions for tier-growth + pop-in.
 *
 * The renderer:
 *   - constructs ONE GrowthFx with the effects layer container,
 *   - calls `dust/rubble/seal(x,y)` at diff time to fire a pooled burst,
 *   - calls `growTransition(container)` / `popIn(container)` on a freshly built
 *     node to animate its scale/alpha in (no geometry rebuild),
 *   - calls `update(dt, viewBounds)` every step,
 *   - calls `clear()` on scene teardown and `dispose()` on destroy.
 */
export class GrowthFx {
  private layer: Container;
  private pool: FxSlot[] = [];
  // Active node transform transitions (tier-grow / pop-in). Keyed by the target
  // container; a fixed-shape record, no per-frame allocation while running.
  private transitions: NodeTransition[] = [];

  constructor(layer: Container) {
    this.layer = layer;
  }

  /** Fire a soft dust puff (tier-grow / pop-in companion). */
  dust(x: number, y: number): void {
    this.fire("dust", x, y);
  }
  /** Fire falling rubble/dust for a removed building. */
  rubble(x: number, y: number): void {
    this.fire("rubble", x, y);
  }
  /** Fire the golden-seal celebration (agent finished here). */
  seal(x: number, y: number): void {
    this.fire("seal", x, y);
  }

  private fire(kind: FxKind, x: number, y: number): void {
    let slot = this.pool.find((s) => !s.active);
    if (!slot) {
      if (this.pool.length < POOL_SIZE) {
        slot = new FxSlot();
        this.layer.addChild(slot.g);
        this.pool.push(slot);
      } else {
        // Pool saturated: recycle the slot with the smallest armSeq — the
        // OLDEST-ARMED burst, not the oldest-created slot. Creation order is NOT
        // arm order (slots are reused), so evicting pool[0] could stomp a young
        // re-armed burst while an older one survives. Alloc-free linear scan over
        // the fixed pool (POOL_SIZE = 16).
        slot = this.pool[0];
        for (let i = 1; i < this.pool.length; i++) {
          if (this.pool[i].armSeq < slot.armSeq) slot = this.pool[i];
        }
        slot.retire();
      }
    }
    slot.arm(kind, x, y);
  }

  /**
   * Animate a freshly-built node growing into place when its file gained tiers.
   * The old silhouette has already been destroyed by the diff; we play the GROW
   * on the NEW (larger) node: it starts a touch smaller + translucent and eases
   * to full size. ~600ms. Pure transform mutation — no geometry rebuild.
   */
  growTransition(container: Container): void {
    this.beginTransition(container, 0.82, 0.35, 600, container.scale.y);
  }

  /**
   * Pop-in for a brand-new building: rise from the ground (scaleY small → full)
   * with a quick fade. ~450ms. Pure transform mutation.
   */
  popIn(container: Container): void {
    this.beginTransition(container, 0.6, 0, 450, container.scale.y);
  }

  private beginTransition(
    container: Container,
    scale0: number,
    alpha0: number,
    durMs: number,
    baseScaleY: number,
  ): void {
    // If this container already has a running transition, replace it (latest
    // delta wins) so a rapid grow→grow doesn't stack.
    const existing = this.transitions.find((t) => t.container === container);
    // CRITICAL: on the replace path the container's scale is ALREADY distorted by
    // the prior transition's starting pose (e.g. 0.6×), so reading it now would
    // capture a shrunken value as the new base and restore the node to that
    // shrunken scale forever. Keep the existing record's already-correct base (the
    // node's true 1× transform captured when the transition first began). Only on
    // a fresh transition do we capture the base from the live (undistorted) node.
    const bSX = existing ? existing.baseSX : container.scale.x;
    const bSY = existing ? existing.baseSY : baseScaleY;
    if (existing) {
      existing.elapsed = 0;
      existing.scale0 = scale0;
      existing.alpha0 = alpha0;
      existing.durMs = durMs;
      // baseSX/baseSY intentionally left untouched — they hold the true base.
    } else {
      this.transitions.push({
        container,
        elapsed: 0,
        scale0,
        alpha0,
        durMs,
        baseSX: bSX,
        baseSY: bSY,
      });
    }
    // Apply the starting pose immediately so there is no one-frame flash at full
    // size before the first update tick. Anchored on the TRUE base.
    container.scale.set(bSX * scale0, bSY * scale0);
    container.alpha = alpha0;
  }

  /**
   * Per-step advance. `viewBounds` is the renderer's reused visible-world
   * Rectangle so off-screen bursts skip their redraw (cull-gated). Bounded work:
   * the active burst set + active transitions are tiny; no allocation.
   */
  update(dt: number, viewBounds: Rectangle): void {
    if (dt <= 0) return;
    // One-shot bursts.
    for (const slot of this.pool) {
      if (!slot.active) continue;
      const visible = viewBounds.contains(slot.x, slot.y);
      slot.step(dt, visible);
    }
    // Node transform transitions (tier-grow / pop-in).
    for (let i = this.transitions.length - 1; i >= 0; i--) {
      const tr = this.transitions[i];
      // A container destroyed mid-transition (e.g. the building changed again
      // and was rebuilt) must be dropped — guard on `destroyed`.
      if ((tr.container as Container & { destroyed?: boolean }).destroyed) {
        this.transitions.splice(i, 1);
        continue;
      }
      tr.elapsed += dt * 1000;
      const t = Math.min(1, tr.elapsed / tr.durMs);
      // easeOutBack-ish: overshoot a hair for a lively "settle".
      const e = 1 - Math.pow(1 - t, 3);
      const scale = tr.scale0 + (1 - tr.scale0) * e;
      tr.container.scale.set(tr.baseSX * scale, tr.baseSY * scale);
      tr.container.alpha = tr.alpha0 + (1 - tr.alpha0) * Math.min(1, t * 1.5);
      if (t >= 1) {
        // Restore exact base transform and finish.
        tr.container.scale.set(tr.baseSX, tr.baseSY);
        tr.container.alpha = 1;
        this.transitions.splice(i, 1);
      }
    }
  }

  /** True if a transition is targeting this container (so the renderer can
   *  reconcile a node that gets rebuilt while growing). */
  hasTransition(container: Container): boolean {
    return this.transitions.some((t) => t.container === container);
  }

  /** Drop a transition targeting `container` and restore its base transform.
   *  Called before the renderer destroys a node that is mid-transition. */
  cancelTransition(container: Container): void {
    const i = this.transitions.findIndex((t) => t.container === container);
    if (i < 0) return;
    const tr = this.transitions[i];
    if (!(tr.container as Container & { destroyed?: boolean }).destroyed) {
      tr.container.scale.set(tr.baseSX, tr.baseSY);
      tr.container.alpha = 1;
    }
    this.transitions.splice(i, 1);
  }

  /** Park every active burst + drop transitions (scene rebuild). Keeps the pool
   *  Graphics for reuse (they live on the effects layer, recreated on clearScene
   *  only if the layer itself is rebuilt). */
  clear(): void {
    for (const slot of this.pool) slot.retire();
    // Restore every in-flight transition to its true base before dropping it.
    // Benign today (clearScene destroys the nodes right after), but without this
    // any future partial-reset that clears effects WITHOUT destroying nodes would
    // freeze every mid-popIn building at a shrunken/translucent pose forever.
    // Mirrors cancelTransition's restore.
    for (const tr of this.transitions) {
      if (!(tr.container as Container & { destroyed?: boolean }).destroyed) {
        tr.container.scale.set(tr.baseSX, tr.baseSY);
        tr.container.alpha = 1;
      }
    }
    this.transitions = [];
  }

  /** Destroy the pool Graphics (renderer destroy). The effects LAYER itself is
   *  destroyed by the renderer; we detach + destroy our slots first so nothing
   *  leaks if the layer is reused. */
  dispose(): void {
    for (const slot of this.pool) {
      slot.g.removeFromParent();
      slot.g.destroy();
    }
    this.pool = [];
    this.transitions = [];
  }
}

interface NodeTransition {
  container: Container;
  elapsed: number;
  scale0: number;
  alpha0: number;
  durMs: number;
  baseSX: number;
  baseSY: number;
}
