import { describe, it, expect } from "vitest";
import { Container, Rectangle } from "pixi.js";
import { GrowthFx, Scaffold } from "./growthEffects";

// These tests exercise the POOLING / RECYCLE / TRANSITION bookkeeping of the L2
// growth-effects layer without a WebGL renderer. PIXI v8 Container/Graphics are
// plain scene-graph objects (no GL needed to construct + mutate transform/alpha
// + record geometry), so we can drive GrowthFx headlessly.

// A wide view so bursts always count as "visible" (the redraw path runs).
const VIEW = new Rectangle(-10000, -10000, 20000, 20000);
// A view that contains nothing (every burst is off-screen → redraw skipped but
// the timer still advances → slot self-recycles).
const OFFSCREEN = new Rectangle(100000, 100000, 1, 1);

describe("GrowthFx — one-shot burst pool", () => {
  it("reuses a single slot for sequential bursts (no growth past 1 in steady use)", () => {
    const layer = new Container();
    const fx = new GrowthFx(layer);
    fx.dust(0, 0);
    expect(layer.children.length).toBe(1); // one pooled slot Graphics created
    // Run it to completion (dust rate 1.6 → ~0.7s of life).
    for (let i = 0; i < 60; i++) fx.update(1 / 30, VIEW);
    // Fire again — the now-idle slot is reused, not a new one.
    fx.rubble(0, 0);
    expect(layer.children.length).toBe(1);
    fx.dispose();
  });

  it("caps the pool at POOL_SIZE under a burst storm", () => {
    const layer = new Container();
    const fx = new GrowthFx(layer);
    // Fire many bursts in the same frame (all active at once).
    for (let i = 0; i < 100; i++) fx.seal(i, i);
    // Pool must not exceed its cap (16).
    expect(layer.children.length).toBeLessThanOrEqual(16);
    expect(layer.children.length).toBeGreaterThan(0);
    fx.dispose();
  });

  it("off-screen bursts still self-recycle (timer advances without redraw)", () => {
    const layer = new Container();
    const fx = new GrowthFx(layer);
    fx.dust(0, 0);
    const slot = layer.children[0];
    expect(slot.visible).toBe(true);
    // Advance with an off-screen view: no redraw, but the slot must finish.
    for (let i = 0; i < 90; i++) fx.update(1 / 30, OFFSCREEN);
    expect(slot.visible).toBe(false); // parked → reusable
    fx.dispose();
  });

  it("spares the most-recently re-armed slot under saturation (FIX 5)", () => {
    const layer = new Container();
    const fx = new GrowthFx(layer);
    // Fill the pool fully (positions 0..15, all active). Creation order == arm
    // order: pool[0] is both the oldest-created AND oldest-armed for now.
    for (let i = 0; i < 16; i++) fx.seal(i, 0);
    expect(layer.children.length).toBe(16);
    const has = (xv: number) =>
      layer.children.some((c) => Math.round(c.position.x) === xv);
    // First saturating fire: both the old oldest-CREATED policy and the new
    // oldest-ARMED policy evict pool[0] (x=0). pool[0] is now re-armed at x=900
    // and is the YOUNGEST-armed slot, while STILL sitting at creation index 0.
    fx.seal(900, 0);
    expect(has(0)).toBe(false); // x=0 was the oldest → evicted
    expect(has(900)).toBe(true); // freshly armed on (creation) index 0
    // Second saturating fire — THE discriminating case:
    //   - OLD policy (evict pool[0], oldest-CREATED) would stomp x=900, the burst
    //     we JUST armed, while the genuinely-old x=1..x=15 survive. BUG.
    //   - NEW policy evicts the min-armSeq slot (x=1, the oldest-ARMED), sparing
    //     the young x=900.
    fx.seal(901, 0);
    expect(layer.children.length).toBe(16); // still capped
    expect(has(900)).toBe(true); // FIX: young re-armed burst survives
    expect(has(1)).toBe(false); // oldest-armed burst was the one dropped
    fx.dispose();
  });

  it("dispose() detaches + frees every pooled slot (no leak)", () => {
    const layer = new Container();
    const fx = new GrowthFx(layer);
    fx.dust(0, 0);
    fx.rubble(5, 5);
    fx.seal(9, 9);
    expect(layer.children.length).toBeGreaterThan(0);
    fx.dispose();
    expect(layer.children.length).toBe(0); // all removed from the layer
  });
});

describe("GrowthFx — node grow/pop-in transitions", () => {
  it("grows a node from a smaller scale back to its base over the duration", () => {
    const layer = new Container();
    const fx = new GrowthFx(layer);
    const node = new Container(); // a stand-in for the building container
    fx.growTransition(node);
    // Starts smaller + translucent.
    expect(node.scale.x).toBeLessThan(1);
    expect(node.alpha).toBeLessThan(1);
    expect(fx.hasTransition(node)).toBe(true);
    // Run past the ~600ms duration.
    for (let i = 0; i < 40; i++) fx.update(1 / 30, VIEW);
    expect(fx.hasTransition(node)).toBe(false);
    // Restored to exact base transform.
    expect(node.scale.x).toBeCloseTo(1, 5);
    expect(node.scale.y).toBeCloseTo(1, 5);
    expect(node.alpha).toBeCloseTo(1, 5);
    fx.dispose();
  });

  it("pop-in starts from the ground (very small) and settles to base", () => {
    const layer = new Container();
    const fx = new GrowthFx(layer);
    const node = new Container();
    fx.popIn(node);
    expect(node.scale.y).toBeLessThan(0.7);
    expect(node.alpha).toBe(0);
    for (let i = 0; i < 30; i++) fx.update(1 / 30, VIEW);
    expect(fx.hasTransition(node)).toBe(false);
    expect(node.scale.y).toBeCloseTo(1, 5);
    expect(node.alpha).toBeCloseTo(1, 5);
    fx.dispose();
  });

  it("a second transition on the same node replaces the first (no stacking, restores TRUE base)", () => {
    const layer = new Container();
    const fx = new GrowthFx(layer);
    const node = new Container();
    fx.popIn(node); // distorts scale to 0.6× / alpha 0 (the starting pose)
    // The node's transform is now distorted. Replacing must NOT capture this
    // distorted scale as the new base — it must keep the true 1× base.
    expect(node.scale.x).toBeLessThan(1);
    fx.growTransition(node); // replaces — must anchor on the true base, not 0.6×
    expect(fx.hasTransition(node)).toBe(true);
    // Only one transition is tracked → after the grow duration it is gone.
    for (let i = 0; i < 40; i++) fx.update(1 / 30, VIEW);
    expect(fx.hasTransition(node)).toBe(false);
    // REGRESSION GUARD (FIX 1): the node must return to its TRUE base transform,
    // not the distorted 0.6× pose captured at replace time.
    expect(node.scale.x).toBeCloseTo(1, 5);
    expect(node.scale.y).toBeCloseTo(1, 5);
    expect(node.alpha).toBeCloseTo(1, 5);
    fx.dispose();
  });

  it("cancelTransition restores the base transform and drops the entry", () => {
    const layer = new Container();
    const fx = new GrowthFx(layer);
    const node = new Container();
    fx.growTransition(node);
    expect(node.scale.x).toBeLessThan(1);
    fx.cancelTransition(node);
    expect(fx.hasTransition(node)).toBe(false);
    expect(node.scale.x).toBeCloseTo(1, 5);
    expect(node.alpha).toBeCloseTo(1, 5);
    fx.dispose();
  });

  it("clear() drops all transitions AND restores their base transform (scene rebuild)", () => {
    const layer = new Container();
    const fx = new GrowthFx(layer);
    const a = new Container();
    const b = new Container();
    fx.growTransition(a);
    fx.popIn(b);
    // Both nodes are now mid-transition with distorted scale/alpha.
    expect(a.scale.x).toBeLessThan(1);
    expect(b.scale.y).toBeLessThan(1);
    expect(b.alpha).toBeLessThan(1);
    fx.clear();
    expect(fx.hasTransition(a)).toBe(false);
    expect(fx.hasTransition(b)).toBe(false);
    // REGRESSION GUARD (FIX 2): clear() must restore every in-flight node to its
    // base transform, otherwise a partial-reset that does not destroy the nodes
    // would leave them frozen invisible/shrunken forever.
    expect(a.scale.x).toBeCloseTo(1, 5);
    expect(a.scale.y).toBeCloseTo(1, 5);
    expect(a.alpha).toBeCloseTo(1, 5);
    expect(b.scale.x).toBeCloseTo(1, 5);
    expect(b.scale.y).toBeCloseTo(1, 5);
    expect(b.alpha).toBeCloseTo(1, 5);
    fx.dispose();
  });
});

describe("Scaffold", () => {
  it("constructs a node and animates its shimmer without throwing", () => {
    const s = new Scaffold(40, 120);
    expect(s.node.children.length).toBeGreaterThan(0); // posts + shimmer
    // A few update steps must not throw (clear+redraw of the shimmer band).
    for (let i = 0; i < 50; i++) s.update(0, 1 / 30);
    s.node.destroy({ children: true });
  });

  it("clamps tiny footprints so geometry stays sane", () => {
    const s = new Scaffold(0, 0); // degenerate inputs
    expect(s.node.children.length).toBeGreaterThan(0);
    s.update(0, 0.05);
    s.node.destroy({ children: true });
  });
});
