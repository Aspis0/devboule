// Shared contract for Polis buildings — the ADAPTER seam between the renderer
// and the ported "Claude Design" procedural kit (../kitcd/*).
//
// `buildBuilding(b, profile, scale)` in index.ts resolves a building's `purpose`
// slug + `visualTier` to one of the kit's 15 builders at a level 0..4, then
// returns a `BuiltBuilding`: ONE display Container (body + animated parts the
// kit builds inside it), a separate ground `shadow`, and the kit's live anim
// instances (Flame/Beacon/Flag/Smoke/Water) that the renderer drives off its
// 30fps step clock — gated to visible chunks only.

import type { Container, Graphics } from "pixi.js";
import type { AnimInstance } from "../kitcd/anims";

/**
 * What `buildBuilding` returns. The kit emits a single container holding the
 * static body Graphics + the animated parts' nodes; we hand that container to
 * the renderer as `display` (positioned at the building's iso point — the kit's
 * makeProj already anchors front-bottom). `shadow` is a separate ground ellipse
 * the renderer parks on its shadows layer. `anims` are the kit's per-frame
 * animated instances (built ONCE; their update() mutates their own small
 * Graphics — the renderer only ticks them for visible chunks).
 */
export interface BuiltBuilding {
  /** The kit container: static body + animated part nodes. Added to a chunk. */
  display: Container;
  /** Ground contact shadow (lives on the shadows layer). */
  shadow: Graphics;
  /** Live animated instances the step clock drives (visible chunks only). */
  anims: AnimInstance[];
  /**
   * TECH LIVERY (F4): a small procedural provider PENNANT (flag glyph) on the
   * roof, built ONCE per building and parented into `display`. `null` when the
   * building has no provider. Static (drawn once, never per-frame), LOD-gated by
   * the renderer (hidden below ~0.5 zoom), and torn down with `display`.
   */
  pennant: Graphics | null;
  /** Half-width of the footprint in px (label x-extent / hit radius). */
  hw: number;
  /** Pixel height of the silhouette above the anchor (label y-offset). */
  depth: number;
  /** Footprint in tiles [W, D] (for sizing / future use). */
  foot: [number, number];
}
