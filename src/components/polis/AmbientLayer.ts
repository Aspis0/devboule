// AmbientLayer.ts — DECORATIVE wandering citizens (Caesar III "walkers").
//
// ┌─────────────────────────────────────────────────────────────────────────┐
// │ PURE-DATA BOUNDARY — READ THIS BEFORE TRUSTING ANYTHING HERE             │
// │                                                                          │
// │ These citizens are SCENERY, not data. They are NEVER part of            │
// │ `city.agents`, never represent a real session, and carry NO real-world  │
// │ meaning. They exist only to make the city feel alive. REAL agents live  │
// │ in `AgentLayer` and are marked with an ARROW above their head; an        │
// │ ambient citizen has NO arrow. If you want real data, look at            │
// │ AgentLayer / city.agents — not here.                                     │
// └─────────────────────────────────────────────────────────────────────────┘
//
// Behaviour: each citizen strolls the REAL road network (the same `roadGraph`
// the agents walk) between random building nodes, pausing now and then like a
// Zeus/Caesar walker. On arrival it picks a new destination. Movement is
// DETERMINISTIC (each walker owns a seeded `Rng` from rng.ts — no Math.random)
// so the scene is reproducible across runs, matching the rest of Polis.
//
// PERFORMANCE: figures are built ONCE per citizen; the step clock only redraws
// the small figure (clear+redraw, exactly like AgentLayer) and nudges position.
// LOD-gated: hidden when zoomed far out. Routes are computed only when a citizen
// actually picks a new destination, never per frame.

import { Container, Graphics, Rectangle, Sprite, type Texture } from "pixi.js";
import { type IsoPoint, isoToCart } from "./iso";
import { Rng, rngFromString, hashString } from "./rng";
import { SlotAllocator, buildSafeSplineLeg, laneOffset, applyPerpendicularOffset, directedLaneOffset, type IPoint, type SafeSplineLeg } from "./locomotion";
import { roundTile } from "./navWalkable";
import {
  drawCitizen,
  defaultTunic,
  shadeColor,
  type CitizenType,
} from "./kitcd/people";
import type { SpriteBank } from "./spriteAssets";

// Match AgentLayer so ambient citizens and real agents read at the same size.
const FIGURE_SCALE = 0.55;
const OMINO_Y_OFFSET = -4;

// A6 — real UH walk-cycle sprites for the ANONYMOUS crowd ("citizen" type
// only). Role-typed ambients (watercarrier/builder/firefighter/noble) keep the
// procedural figures: their accessories (yoke, hammer, bucket) are semantic —
// they visually pre-announce the claimable roles — and the UH set has no
// equivalents. 8 directions × 4 frames per sex; textures resolved ONCE at
// construction into a lookup array so the per-step swap allocates nothing.
//
// Direction convention (read off the UH frames): folder = compass with 0=W,
// 90=N, 180=E, 270=S. Screen movement angle θ (atan2(dy,dx), y-down) maps to
// folder (180+θ) mod 360 — bucket k = round(θ/45) mod 8 indexes this table.
const WALK_DIR_FOLDERS = [180, 225, 270, 315, 0, 45, 90, 135] as const;
const WALK_FRAMES = 4;
// UH citizen canvas is 24×40 (figure ~34px + baked shadow); the procedural
// figure is ~23px at FIGURE_SCALE 0.55 ≈ 13px. 0.42 lands the sprite at a
// matching on-screen height.
const WALK_SPRITE_SCALE = 0.42;
/** Per-sex direction×frame texture table (null ⇒ procedural fallback).
 *  `anchor` comes from the manifest (the pipeline's authoritative feet
 *  registration), read once — never hardcoded at the Sprite. */
type WalkFrameTable = {
  m: Texture[][];
  f: Texture[][];
  anchor: readonly [number, number];
} | null;

/**
 * A6 — direction bucket (0..7 into WALK_DIR_FOLDERS) for a screen-space
 * movement delta (y-down). Pure so the octant math is unit-testable: bucket 0
 * = east, 2 = south (toward the camera), 4 = west, 6 = north.
 */
export function walkDirBucket(dx: number, dy: number): number {
  const theta = Math.atan2(dy, dx);
  return ((Math.round(theta / (Math.PI / 4)) % 8) + 8) % 8;
}

/**
 * Resolve the crowd walk-cycle textures from the bank ONCE. Returns null (⇒
 * every walker stays procedural) unless BOTH sexes have ALL 8×4 frames — a
 * partial set would leave some directions frozen mid-stride.
 */
export function buildWalkFrameTable(
  bank: SpriteBank | null | undefined,
): WalkFrameTable {
  if (!bank) return null;
  const out = {
    m: [] as Texture[][],
    f: [] as Texture[][],
    anchor: bank.anchor("walk:citizenm:270:f0"),
  };
  for (const sex of ["m", "f"] as const) {
    for (let k = 0; k < WALK_DIR_FOLDERS.length; k++) {
      const frames: Texture[] = [];
      for (let f = 0; f < WALK_FRAMES; f++) {
        const tex = bank.get(`walk:citizen${sex}:${WALK_DIR_FOLDERS[k]}:f${f}`);
        if (!tex) return null;
        frames.push(tex);
      }
      out[sex].push(frames);
    }
  }
  return out;
}

// Strolling pace — deliberately slower than agents (70px/s) so the crowd
// ambles rather than marches.
const WALK_SPEED = 42; // iso px/s

// Idle pause between strolls (ms), picked per-stop in [MIN, MIN+SPAN).
const PAUSE_MIN_MS = 500;
const PAUSE_SPAN_MS = 1800;

// Discrete bob offsets (px), cycled on the step clock (same cadence as agents).
const BOB_OFFSETS = [0, -1, -2, -1] as const;

// Ambient citizens read as background: a touch more transparent than the real
// agents (which are full-alpha + arrow) so the live agents always pop.
const AMBIENT_ALPHA = 0.88;

// Crowd size relative to the road-connected building count, capped for perf.
// Phase 4: raised to 64 so the rich profile's maxAmbientWalkers (64) is the
// effective ceiling; weaker tiers still clamp via desiredAmbientCount's cap arg.
const AMBIENT_PER_NODE = 0.4;
const MAX_AMBIENT = 64;
const MIN_AMBIENT = 4;

// Figure vocabulary for the DECORATIVE ambient crowd. Polis-P2 broadens this to
// the 5 CLAIMABLE citizen types so the idle population visibly contains a
// reference omino of EVERY claimable role — a coder-type builder (and a
// Censor-type firefighter) roams even before any real agent of that role
// activates, and P3's "possession" can adopt an existing ambient walker.
// Only `merchant` stays EXCLUDED:
//   - `merchant` — DEDICATED to the DATA-BOUND trade-route porters (TradeRouteLayer):
//     a merchant figure carries a goods sack and walks a REAL import road from the
//     supplier to the consumer. Keeping it OUT of the decorative crowd means a
//     merchant-with-sack on a road is UNAMBIGUOUSLY a data-bound trade porter,
//     distinct from this scenery crowd and from real agents (gold arrow).
// NOTE: an idle `firefighter` draws its bucket only (actionPhase 0 ⇒ no water
// arc), so it reads as a plain bucket-carrier while strolling — the water-throw
// is reserved for the Censor presence at a building (P5).
// (Honors "those six figures" across the city while keeping each read distinct.)
// Object.freeze so a consumer can't mutate the shared array at runtime (the
// `readonly` type is compile-time only; this is indexed by `index % length` in
// spawn(), so an accidental push/splice would skew the whole crowd vocabulary).
export const AMBIENT_TYPES: readonly CitizenType[] = Object.freeze([
  "citizen",
  "watercarrier",
  "noble",
  "builder",
  "firefighter",
]);

// Forum bustle: a few extra DECORATIVE citizens that linger near the civic
// buildings (market / townhall / commons) so those squares read as busy. They
// are a small fraction of the crowd, longer-pausing, and bias their wander
// targets toward the forum anchors. Still pure scenery — never `city.agents`.
const FORUM_FRACTION = 0.35; // of the crowd, when forum anchors exist
const MAX_FORUM = 10; // hard cap on lingerers
// Lingerers pause longer (they "mill") — a wider, later idle window.
const FORUM_PAUSE_MIN_MS = 1400;
const FORUM_PAUSE_SPAN_MS = 3200;
// How strongly a forum lingerer prefers a forum anchor as its next stop. The
// rest of the time it takes a normal weighted wander so it doesn't ping-pong.
const FORUM_TARGET_BIAS = 0.7;

type WalkerState =
  // Standing at `pos`, counting down to the next stroll.
  | { kind: "idle"; remainingMs: number }
  // Walking `route[seg]`→`route[seg+1]`, `t` in [0,1), ending at `toNode`.
  | { kind: "walk"; route: IsoPoint[]; seg: number; t: number; toNode: string; destFileId: string };

interface Walker {
  /** Per-citizen deterministic RNG (seeded from the walker index). */
  rng: Rng;
  type: CitizenType;
  tunic: number;
  /** Container at `pos` (bobbed vertically, flipped by facing). */
  container: Container;
  /** The figure Graphics, cleared + redrawn each visible step. */
  base: Graphics;
  /** A6 — UH walk-cycle sprite (anonymous "citizen" type only; null ⇒ the
   *  procedural drawCitizen path). Texture swapped per step, never redrawn. */
  sprite: Sprite | null;
  /** A6 — which sex's frame table drives `sprite` ("m" | "f"). */
  spriteSex: "m" | "f";
  /** A6 — current direction bucket (0..7) into WALK_DIR_FOLDERS. */
  dirIdx: number;
  /** Current ISO anchor (feet) of the citizen. */
  pos: IsoPoint;
  /** Building node the citizen is standing at / last departed from. */
  nodeId: string;
  /** Walk-cycle phase (radians) — advances while moving (legs/arms swing). */
  phase: number;
  /** Per-citizen integer offset for the stepped bob cadence (0..3). */
  bobPhase: number;
  /** Horizontal facing (+1 right, -1 left). */
  facing: number;
  /** P5.2 — per-walker lane offset px (deterministic from rng). */
  laneOff: number;
  /** DECORATIVE "forum lingerer": pauses longer + biases its next stop toward a
   *  civic anchor (market/townhall/commons). Pure scenery, like every walker. */
  forum: boolean;
  /** P5.2 — idle variant: "none" | "lookAround" | "sit". Seeded per walker. */
  idleVariant: "none" | "lookAround" | "sit";
  /** P5.2 — stable walker id for slot allocation (does NOT change on move). */
  wid: string;
  state: WalkerState;
}

type Resolve = (fileId: string) => IsoPoint | null;
type FindRoute = (fromFileId: string, toFileId: string) => IsoPoint[] | null;

/**
 * Desired ambient crowd size for a city with `nodeCount` road-connected nodes.
 *
 * B2c: an optional `maxWalkers` (the hardware render profile's cap) LOWERS the
 * ceiling below the default {@link MAX_AMBIENT} on a weaker tier — the effective
 * cap is `min(MAX_AMBIENT, maxWalkers)`. The MIN floor is also clamped to that cap
 * so a minimal tier asking for (say) 6 is never overridden back up to the default
 * floor. Omitted/non-finite → the historical {@link MAX_AMBIENT} ceiling.
 */
export function desiredAmbientCount(
  nodeCount: number,
  maxWalkers?: number,
): number {
  if (nodeCount <= 0) return 0;
  const cap =
    typeof maxWalkers === "number" && Number.isFinite(maxWalkers)
      ? Math.max(0, Math.min(MAX_AMBIENT, Math.floor(maxWalkers)))
      : MAX_AMBIENT;
  const scaled = Math.floor(nodeCount * AMBIENT_PER_NODE);
  // Floor never exceeds the cap (a tight cap wins over MIN_AMBIENT).
  const floor = Math.min(MIN_AMBIENT, cap);
  return Math.max(floor, Math.min(cap, scaled));
}

/** The minimal walker shape {@link pickNearestIdle} needs — a structural subset
 *  of {@link Walker} so the selection is unit-testable headlessly (no PIXI). */
export interface ClaimableWalker {
  type: CitizenType;
  pos: IsoPoint;
  /** Only the discriminant matters for claimability (idle == standing at node). */
  state: { kind: "idle" | "walk" };
}

/**
 * Polis-P3 — PURE selection for the claim handoff: index of the NEAREST CLAIMABLE
 * ambient walker of `figureType` to `nearIso`, or -1 if none.
 *
 * "Claimable" means the walker is IDLE (`state.kind === "idle"`) — i.e. standing
 * AT its road node, NOT mid-segment. A mid-walk walker is rejected so the handoff
 * position P4/AgentLayer receives is a clean node anchor (not a fractional point
 * along a road polyline that would leave the claimed agent floating off-graph).
 * Other figure types are ignored. Distance is straight ISO euclidean.
 *
 * Deterministic: on an exact distance tie the LOWER index wins (stable scan), so
 * the same crowd + nearIso always selects the same walker.
 */
export function pickNearestIdle(
  walkers: readonly ClaimableWalker[],
  figureType: CitizenType,
  nearIso: IsoPoint,
): number {
  let best = -1;
  let bestDistSq = Infinity;
  for (let i = 0; i < walkers.length; i++) {
    const w = walkers[i];
    if (w.type !== figureType) continue;
    if (w.state.kind !== "idle") continue; // mid-walk == not a clean handoff
    const dx = w.pos.x - nearIso.x;
    const dy = w.pos.y - nearIso.y;
    const d2 = dx * dx + dy * dy;
    // Strict `<` so an exact tie keeps the earlier (lower) index — deterministic.
    if (d2 < bestDistSq) {
      bestDistSq = d2;
      best = i;
    }
  }
  return best;
}

export class AmbientLayer {
  private root: Container;
  private walkers: Walker[] = [];
  private visible = true;
  // Polis-P3 — number of ambient walkers currently CLAIMED by an activating agent
  // (handed off to AgentLayer via `release`). The effective ambient target is
  // shrunk by this offset so a claimed walker is NOT auto-respawned by setCount()
  // while P4 owns it; `adopt` decrements it when the agent's omino returns to the
  // crowd. Single-owner invariant: a walker is owned by EITHER this layer OR
  // AgentLayer, never both — the claimedCount tracks how many we've handed away.
  // P5.2 — shared per-building entry-slot allocator (presentation, NOT CityState).
  private slotAllocator: SlotAllocator;
  // T2 — walk blocker: true on tiles walkers must never stand on (water/buildings).
  // Built once in the renderer's world-setup and passed here. Identity-stable.
  private blocked: (gx: number, gy: number) => boolean;
  private claimedCount = 0;
  // Monotonic counter feeding a DETERMINISTIC seed for each `adopt`ed walker, so a
  // re-inserted omino roams reproducibly without Math.random/Date.now. Combined
  // with the snapped node id + type so two adopts at the same spot still differ.
  private adoptSeq = 0;

  // World accessors (updated by setWorld); empty until a city with roads loads.
  private nodeIds: string[] = [];
  private resolve: Resolve | null = null;
  private findRoute: FindRoute | null = null;
  // CUMULATIVE per-node weight (aligned with `nodeIds`): cumWeight[i] is the
  // running total of node weights up to and including node i, so a weighted pick
  // is one rng.float() + a binary search. Rebuilt once per setWorld, never per
  // pick. Decorative-only — biases foot traffic toward busy arterials.
  private cumWeight: number[] = [];
  private totalWeight = 0;
  // Civic "forum" anchor node ids (market / townhall / commons) the lingerers
  // prefer. A subset of `nodeIds`; empty when the city has none. Scenery only.
  private forumNodeIds: string[] = [];

  // A6 — prebuilt walk-cycle texture table (null ⇒ all-procedural crowd).
  private walkFrames: WalkFrameTable = null;

  constructor(
    root: Container,
    slotAllocator?: SlotAllocator,
    blocked?: (gx: number, gy: number) => boolean,
    spriteBank?: SpriteBank | null,
  ) {
    this.root = root;
    this.slotAllocator = slotAllocator ?? new SlotAllocator();
    this.blocked = blocked ?? (() => false);
    this.walkFrames = buildWalkFrameTable(spriteBank);
  }

  /**
   * A6 — drop the walk-frame table. Called from PolisRenderer.destroy() ONLY
   * (the same lifecycle moment as setKitSpriteBank(null)): right after it,
   * createPolis unloads the bank's Assets urls, so the table's Textures die —
   * a retained reference would hand any late caller dead GPU backings.
   * Deliberately NOT part of clear(): clear() runs on every city rebuild
   * (folder switch), where the bank is still live and the next crowd must
   * keep its sprites.
   */
  dropSpriteBank(): void {
    this.walkFrames = null;
  }

  /**
   * Point the ambient crowd at the current city's walkable graph. Existing
   * walkers KEEP strolling (so a live file-diff doesn't reset the crowd); they
   * just use the new graph on their next decision. An empty graph clears all.
   *
   * @param nodeWeights per-node "busy-ness" weight aligned with `nodeIds` (sum of
   *        incident road weights). Biases the DECORATIVE crowd toward arterials.
   * @param forumNodeIds civic anchor node ids (market/townhall/commons) the
   *        forum lingerers prefer. A subset of `nodeIds`; may be empty.
   */
  setWorld(
    nodeIds: string[],
    resolve: Resolve,
    findRoute: FindRoute,
    nodeWeights: number[] = [],
    forumNodeIds: string[] = [],
  ): void {
    this.nodeIds = nodeIds;
    this.resolve = resolve;
    this.findRoute = findRoute;
    this.forumNodeIds = forumNodeIds;
    // Build the cumulative-weight table once. If weights are missing/mismatched
    // we fall back to UNIFORM (weight 1 each) so routing still works.
    const cum: number[] = [];
    let running = 0;
    for (let i = 0; i < nodeIds.length; i++) {
      const w = nodeWeights.length === nodeIds.length ? Math.max(0, nodeWeights[i]) : 1;
      running += w > 0 ? w : 1;
      cum.push(running);
    }
    this.cumWeight = cum;
    this.totalWeight = running;
    if (nodeIds.length === 0) {
      this.clear();
      return;
    }
    // FIX: a live diff (building/road removed) can rebuild the graph such that a
    // walker's current `nodeId` no longer exists. Such a walker would get `null`
    // from every findRoute and idle forever at a now-floating position. Re-seat
    // any orphaned walker onto a valid node and force an immediate re-pick so it
    // resumes strolling. Deterministic: each walker re-seats via its OWN seeded
    // rng (weighted pick over the new graph), and we snap its position through
    // `resolve`. If a walker can't be resolved onto any node we leave it idle so
    // it retries cheaply next tick (never crashes; never invents a path).
    const valid = new Set(nodeIds);
    for (const w of this.walkers) {
      // A mid-walk walker whose DESTINATION node vanished is also orphaned — its
      // route waypoints are stale geometry from the old graph.
      const stuck =
        !valid.has(w.nodeId) ||
        (w.state.kind === "walk" && !valid.has(w.state.toNode));
      if (!stuck) continue;
      const reseat = this.pickWeightedNode(w.rng);
      const p = reseat ? resolve(reseat) : null;
      if (reseat && p) {
        w.nodeId = reseat;
        w.pos = { x: p.x, y: p.y };
        this.applyTransform(w);
      }
      // Force an immediate re-pick on the next update() tick (idle, 0 remaining)
      // regardless of whether the snap succeeded, so a transiently unresolvable
      // walker keeps retrying instead of riding a stale walk route.
      w.state = { kind: "idle", remainingMs: 0 };
    }
  }

  /** Grow/shrink the crowd to `n` walkers (delta only — never a full rebuild). */
  setCount(n: number): void {
    if (!this.resolve || this.nodeIds.length === 0) {
      // No world to walk: tear the crowd down rather than park ghosts.
      if (this.walkers.length > 0) this.clear();
      return;
    }
    // Polis-P3 — the LIVE crowd target is the requested size MINUS the walkers
    // currently claimed by AgentLayer (handed off via `release`). Without this
    // offset a claimed walker would be immediately respawned here, double-counting
    // it (one omino as a real agent + one fresh ambient clone). Clamp the offset
    // to the request so an over-claim can never ask for a negative crowd.
    const claimed = Math.min(this.claimedCount, Math.max(0, n));
    const target = Math.max(0, Math.min(n - claimed, MAX_AMBIENT));
    while (this.walkers.length > target) {
      const w = this.walkers.pop();
      if (w) this.destroyWalker(w);
    }
    while (this.walkers.length < target) {
      const w = this.spawn(this.walkers.length);
      if (w) this.walkers.push(w);
      else break; // could not find any resolvable node — stop trying
    }
  }

  /** T2 — Update the walk blocker predicate (called once per city load). */
  setBlocked(blocked: (gx: number, gy: number) => boolean): void {
    this.blocked = blocked;
  }

  setLodVisible(visible: boolean): void {
    this.visible = visible;
    for (const w of this.walkers) w.container.visible = visible;
  }

  /** Number of ambient citizens currently in the crowd. */
  get count(): number {
    return this.walkers.length;
  }

  /** Polis-P3 — how many ambient walkers are currently CLAIMED (handed off to
   *  AgentLayer via `release` and not yet returned via `adopt`). Exposed so P4
   *  and tests can assert the accounting offset. */
  get claimed(): number {
    return this.claimedCount;
  }

  // -------------------------------------------------------------------------
  // Polis-P3 — CLAIM primitives. An activating agent can take possession of an
  // idle roaming omino (`release`) and later return it to the crowd (`adopt`).
  // Single-owner invariant: a walker is owned by EITHER this layer OR AgentLayer,
  // never both. `release` REMOVES the walker from the ambient crowd (full PIXI
  // teardown — no leak) and hands AgentLayer its clean node-anchored handoff
  // state; `adopt` re-inserts a fresh roaming walker. The `claimedCount` offset
  // keeps the crowd target honest so a claimed walker is never auto-respawned.
  // -------------------------------------------------------------------------

  /**
   * Take possession of the NEAREST idle ambient walker of `figureType` to
   * `nearIso`. Removes it from the crowd (proper PIXI teardown) and returns its
   * handoff state — its current position and the road node it is standing at —
   * so AgentLayer (P4) can start the claimed agent from exactly there.
   *
   * Returns null when no IDLE walker of that type exists; P4 then falls back to
   * spawning a fresh agent omino. Increments `claimedCount` so the released
   * walker's slot is NOT auto-respawned by a subsequent setCount() while claimed.
   */
  release(
    figureType: CitizenType,
    nearIso: IsoPoint,
  ): { pos: IsoPoint; nodeId: string } | null {
    const idx = pickNearestIdle(this.walkers, figureType, nearIso);
    if (idx < 0) return null;
    const w = this.walkers[idx];
    // Snapshot the handoff BEFORE teardown (a destroyed walker's pos is still a
    // plain object, but copy defensively so the caller owns immutable state).
    const handoff = { pos: { x: w.pos.x, y: w.pos.y }, nodeId: w.nodeId };
    // Remove from the crowd. swap-remove is fine: walker order carries no meaning
    // (spawn index only seeds construction, never indexes the live array later).
    const last = this.walkers.pop();
    if (last && last !== w) this.walkers[idx] = last;
    this.destroyWalker(w); // full PIXI cleanup — no leak
    // The claimed slot is now owned by AgentLayer: shrink the effective crowd
    // target so setCount() won't respawn it.
    this.claimedCount += 1;
    return handoff;
  }

  /**
   * Return a previously-claimed omino of `figureType` to the roaming crowd at /
   * near `pos` (snapped to the nearest road node) and resume free roaming from
   * there. Restores the `claimedCount` offset so the crowd is back to its target.
   *
   * Determinism: the re-inserted walker's RNG is seeded from a monotonic adopt
   * counter combined with the snapped node id and figure type (NOT Math.random /
   * Date.now), so the same sequence of adopts replays identically.
   *
   * No-op (other than restoring the offset, floored at 0) if there is no world to
   * walk or no resolvable node near `pos` — the omino simply isn't re-added, and
   * the offset is undone so the crowd's target self-heals on the next setCount().
   */
  adopt(figureType: CitizenType, pos: IsoPoint | null): void {
    // Restore the accounting first so it's undone even on an early return below.
    if (this.claimedCount > 0) this.claimedCount -= 1;
    // Polis-P6 FIX (#5) — a NULL pos means the caller could not honestly locate the
    // returning omino (no building anchor AND no live agent position). Do NOT
    // fabricate {0,0}: that would snap a returned walker to a random near-origin
    // node (spatially dishonest, omino teleports across the map). The claimedCount
    // decrement above already balanced the claim; we simply skip the re-insertion,
    // and the crowd self-heals to target on the next setCount(). Same no-op outcome
    // the unresolvable-node branch already takes — just honest about position.
    if (!pos || !this.resolve || this.nodeIds.length === 0) return;
    const nodeId = this.nearestNodeId(pos);
    const snapped = nodeId ? this.resolve(nodeId) : null;
    if (!nodeId || !snapped) return; // can't honestly seat it — leave it out
    const seq = this.adoptSeq++;
    const rng = new Rng(hashString(`adopt:${figureType}:${nodeId}:${seq}`));
    // A re-inserted omino is a plain passer-by (never a forum lingerer — that
    // role is index-derived for the base crowd) and resumes roaming promptly.
    const w = this.buildWalkerAt(figureType, nodeId, snapped, rng, {
      forum: false,
      bobPhase: seq % BOB_OFFSETS.length,
      remainingMs: 200,
    });
    // Respect the MAX_AMBIENT perf cap. During a claim window walkers can be
    // released and setCount() can re-fill the crowd; multiple concurrent adopts
    // could otherwise push past the cap (transient over-target). If we're already
    // at the cap, the claimed slot is still accounted for (claimedCount was just
    // decremented), so we tear the built walker down — no PIXI leak — and the next
    // setCount() fills the slot normally. This also closes the n < claimedCount
    // overflow (same root cause).
    if (this.walkers.length < MAX_AMBIENT) {
      this.walkers.push(w);
    } else {
      this.destroyWalker(w);
    }
  }

  /** Nearest graph node (by ISO distance) to an arbitrary point, or null when
   *  the graph is empty / nothing resolves. Used to SNAP an adopted omino back
   *  onto the real road network so it resumes routing cleanly. Deterministic:
   *  a strict `<` keeps the first (graph-order) node on an exact tie. */
  private nearestNodeId(iso: IsoPoint): string | null {
    // O(N) linear scan over all nodes. Fine at the current MAX_AMBIENT=40 scale
    // (adopt is rare — only on a claim handoff). A spatial index (grid/k-d tree)
    // is a future optimization should the graph grow to thousands of nodes.
    const resolve = this.resolve;
    if (!resolve) return null;
    let best: string | null = null;
    let bestDistSq = Infinity;
    for (const id of this.nodeIds) {
      const p = resolve(id);
      if (!p) continue;
      const dx = p.x - iso.x;
      const dy = p.y - iso.y;
      const d2 = dx * dx + dy * dy;
      if (d2 < bestDistSq) {
        bestDistSq = d2;
        best = id;
      }
    }
    return best;
  }

  // -------------------------------------------------------------------------
  // Per-step (retro cadence): bob + figure redraw. LOD-gated. No allocation.
  //
  // Polis-P6 PERF LOCK: step() NEVER routes. findRoute is called ONLY from
  // pickNextTarget — i.e. on a STOP BOUNDARY (a walker's idle timer expired and it
  // chooses a new destination), an EVENT, not per frame. update() integrates the
  // PRE-COMPUTED route polyline (advanceWalk). So routing is event-driven (a handful
  // of picks as walkers arrive), never a per-frame flood over the whole crowd.
  // -------------------------------------------------------------------------
  step(frame: number, view?: Rectangle): void {
    if (!this.visible) return;
    for (const w of this.walkers) {
      // NOTE: no early `!w.container.visible` continue here — while LOD-visible the
      // only reason a container is hidden is the viewport cull below, which must be
      // re-evaluated each step so an off-screen walker that scrolls back IN is
      // re-shown + redrawn (an early continue would strand it hidden forever).
      // #9 — viewport cull: skip the clear+redraw for a walker whose pos is outside
      // the camera bounds (a margin covers the figure's own height). Mirrors
      // TradeRouteLayer.step: the off-screen walker is HIDDEN and re-shown + redrawn
      // the step it scrolls back into view (never left blank on re-entry). Position
      // is still advanced in update() (movement is uncoupled from the redraw), so a
      // walker that crosses the screen keeps a correct position the whole time.
      // `view` is optional (headless tests step without one → LOD-only, no cull).
      if (view && !AmbientLayer.inView(w.pos, view)) {
        if (w.container.visible) w.container.visible = false;
        continue;
      }
      if (!w.container.visible) w.container.visible = true;
      const bob = BOB_OFFSETS[(Math.floor(frame / 2) + w.bobPhase) % BOB_OFFSETS.length];
      w.container.position.y = w.pos.y + OMINO_Y_OFFSET + bob;

      const moving = w.state.kind === "walk";
      // P5.2 — idle variants.
      //   lookAround: slow phase creep for a subtle idle sway (moving=false so
      //     drawCitizen keeps legs static; the evolving phase feeds a future
      //     head-turn drawing branch — TODO(P5.2) add head-turn draw logic to
      //     kitcd/people.ts).
      //   sit: shifts the figure down 6px (no new drawing pipeline).
      let drawPhase = w.phase;
      let yShift = 0;
      if (!moving && w.idleVariant === "lookAround") {
        drawPhase += 0.03;
        w.phase += 0.03;
      } else if (!moving && w.idleVariant === "sit") {
        yShift = 6;
      }
      if (moving) w.phase += 0.6; // legs/arms swing while strolling
      w.container.position.y = w.pos.y + OMINO_Y_OFFSET + bob + yShift;
      if (w.sprite && this.walkFrames) {
        // A6 — sprite crowd: swap the walk-cycle frame (phase advances 0.6 per
        // step ⇒ one frame per step tick), direction from the current route
        // leg (dirIdx, updated in advanceWalk). Idle stands on frame 0. A
        // texture assignment is a reference swap — no allocation, no redraw.
        const frames = this.walkFrames[w.spriteSex][w.dirIdx];
        const frame = moving
          ? Math.floor(w.phase / 0.6) % WALK_FRAMES
          : 0;
        const tex = frames[frame];
        if (w.sprite.texture !== tex) w.sprite.texture = tex;
      } else {
        drawCitizen(w.base, w.type, {
          moving, // false for idle variants (no leg swing)
          phase: drawPhase,
          actionPhase: 0, // ambient citizens never perform actions
          tunic: w.tunic,
        });
      }
    }
  }

  /** #9 — is `pos` inside the camera's visible-world rectangle (with a margin that
   *  covers a figure's own height/width)? Allocation-free; mirrors the cull margin
   *  TradeRouteLayer.step uses so the crowd layers cull consistently. */
  private static inView(pos: IsoPoint, view: Rectangle): boolean {
    const M = 48;
    return (
      pos.x >= view.x - M &&
      pos.x <= view.x + view.width + M &&
      pos.y >= view.y - M &&
      pos.y <= view.y + view.height + M
    );
  }

  // -------------------------------------------------------------------------
  // Per-frame (smooth, real ms): advance the idle timer / walk integration.
  // -------------------------------------------------------------------------
  update(deltaMs: number): void {
    if (deltaMs <= 0) return;
    // Polis-P6 — LOD halt: when zoomed out past LOD_AGENTS the crowd is hidden, so
    // skip ALL per-frame movement integration too (not just the redraw in step()).
    // Nothing in the walker system should advance while invisible — no position
    // lerp, no idle countdown, no pickNextTarget (and therefore no findRoute). The
    // crowd simply freezes; it resumes from its frozen state on zoom-in. Cheap: a
    // single boolean test in the zoomed-out steady state.
    if (!this.visible) return;
    for (const w of this.walkers) {
      if (w.state.kind === "idle") {
        w.state.remainingMs -= deltaMs;
        if (w.state.remainingMs <= 0) this.pickNextTarget(w);
      } else {
        this.advanceWalk(w, w.state, deltaMs);
      }
    }
  }

  // -------------------------------------------------------------------------
  // Movement
  // -------------------------------------------------------------------------

  /** Choose a new destination node and start walking there; idle again if no
   *  real road route exists (we never invent a path).
   *
   *  Destinations are WEIGHT-BIASED: each pick is sampled from the per-node
   *  cumulative-weight table so busy arterials (more/heavier incident roads)
   *  draw proportionally more foot traffic — the city's avenues read lively.
   *  Forum lingerers additionally bias toward a civic anchor most of the time.
   *  All sampling uses the walker's seeded rng, so the crowd is reproducible. */
  private pickNextTarget(w: Walker): void {
    if (!this.findRoute || this.nodeIds.length < 2) {
      w.state = { kind: "idle", remainingMs: this.pauseMs(w) };
      return;
    }
    // A few seeded attempts to find a connected destination.
    for (let attempt = 0; attempt < 4; attempt++) {
      // Forum lingerers prefer a civic anchor most of the time (when any exist);
      // everyone else (and a forum walker the rest of the time) picks weighted by
      // road busy-ness so arterials get the crowd.
      const useForum =
        w.forum &&
        this.forumNodeIds.length > 0 &&
        w.rng.float() < FORUM_TARGET_BIAS;
      const target = useForum
        ? w.rng.pick(this.forumNodeIds)
        : this.pickWeightedNode(w.rng);
      if (!target || target === w.nodeId) continue;
      const route = this.findRoute(w.nodeId, target);
      if (route && route.length >= 2) {
        // Snap the start to the route's first waypoint so the figure doesn't
        // jump, and walk to the destination anchor.
        w.pos = { x: route[0].x, y: route[0].y };
        w.state = { kind: "walk", route, seg: 0, t: 0, toNode: target, destFileId: target };
        return;
      }
    }
    // Disconnected or unlucky this tick — pause and retry later.
    w.state = { kind: "idle", remainingMs: this.pauseMs(w) };
  }

  /**
   * Weighted node pick: sample a node from `nodeIds` with probability
   * proportional to its busy-ness weight, via one rng.float() + a binary search
   * over the prebuilt cumulative-weight table (no per-pick allocation). Falls
   * back to a plain uniform pick if the table is empty/degenerate. DECORATIVE —
   * biases foot traffic only; carries no data meaning.
   */
  private pickWeightedNode(rng: Rng): string | null {
    const n = this.nodeIds.length;
    if (n === 0) return null;
    if (this.totalWeight <= 0 || this.cumWeight.length !== n) {
      return rng.pick(this.nodeIds);
    }
    const r = rng.float() * this.totalWeight;
    // Binary search for the first cumulative bucket > r.
    let lo = 0;
    let hi = n - 1;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (this.cumWeight[mid] <= r) lo = mid + 1;
      else hi = mid;
    }
    return this.nodeIds[lo];
  }

  /**
   * P5.2 — advance walk with Catmull-Rom spline easing + lane offset.
   */
  private advanceWalk(
    w: Walker,
    m: { kind: "walk"; route: IsoPoint[]; seg: number; t: number; toNode: string; destFileId: string },
    deltaMs: number,
  ): void {
    let remaining = (WALK_SPEED * deltaMs) / 1000;
    const route = m.route;
    while (remaining > 0 && m.seg < route.length - 1) {
      const a = route[m.seg];
      const b = route[m.seg + 1];
      const segLen = Math.hypot(b.x - a.x, b.y - a.y) || 1e-6;
      const distLeft = segLen * (1 - m.t);
      if (remaining < distLeft) {
        m.t += remaining / segLen;
        remaining = 0;
      } else {
        remaining -= distLeft;
        m.seg += 1;
        m.t = 0;
      }
    }

    if (m.seg >= route.length - 1) {
      // Arrived.
      const end = route[route.length - 1];
      this.faceTowards(w, route[Math.max(0, route.length - 2)], end);
      // P5.2 — release old slot, acquire new entry slot.
      this.slotAllocator.release(w.nodeId, w.wid);
      const slotIdx = this.slotAllocator.acquire(m.toNode, w.wid);
      const secondLast = route[Math.max(0, route.length - 2)];
      const dir = { x: end.x - secondLast.x, y: end.y - secondLast.y };
      const slotPos = this.slotAllocator.positionFor(slotIdx, end, dir);
      // T2 — validate slot position: convert ISO→cartesian before querying blocker.
      const slotCart = isoToCart(slotPos.x, slotPos.y);
      if (this.blocked(roundTile(slotCart.x), roundTile(slotCart.y))) {
        w.pos = { x: end.x, y: end.y };
      } else {
        w.pos = { x: slotPos.x, y: slotPos.y };
      }
      w.nodeId = m.toNode;
      w.state = { kind: "idle", remainingMs: this.pauseMs(w) };
      this.applyTransform(w);
      return;
    }

    // Mid-route: Catmull-Rom spline + lane offset.
    // T2 — use buildSafeSplineLeg to validate the spline against blocked tiles
    // and clamp the lane offset if the road hugs a shore.
    const a = route[m.seg];
    const b = route[m.seg + 1];
    // M2: cache the safe spline result per leg; rebuild only on seg change.
    const splineKey = m.seg;
    const cached = (m as any)._spline as { key: number; result: SafeSplineLeg } | undefined;
    let safe: SafeSplineLeg;
    if (cached && cached.key === splineKey) {
      safe = cached.result;
    } else {
      safe = buildSafeSplineLeg(route as IPoint[], m.seg, this.blocked);
      (m as any)._spline = { key: splineKey, result: safe };
    }
    const raw = safe.sample(m.t);
    const dx = b.x - a.x;
    const dy = b.y - a.y;
    // T2 — if the lane offset is clamped for this leg, use 0.
    const laneOff = safe.laneOffsetClamped
      ? 0
      : directedLaneOffset(w.wid, dx, dy);
    const off = applyPerpendicularOffset(raw, dx, dy, laneOff);
    w.pos = { x: off.x, y: off.y };
    this.faceTowards(w, a, b);
    this.applyTransform(w);
  }

  private faceTowards(w: Walker, a: IsoPoint, b: IsoPoint): void {
    const dx = b.x - a.x;
    const dy = b.y - a.y;
    // A6 — sprite walkers encode direction in the FRAME (8 real facings), so
    // the container must never mirror (scale.x stays 1; a flip would swap
    // left/right facings AND mirror the baked shadow). Procedural figures
    // keep the classic 2-way mirror.
    if (w.sprite) {
      if (dx * dx + dy * dy > 1e-4) w.dirIdx = walkDirBucket(dx, dy);
      return;
    }
    if (dx > 0.01) w.facing = 1;
    else if (dx < -0.01) w.facing = -1;
    w.container.scale.x = w.facing;
  }

  private applyTransform(w: Walker): void {
    w.container.position.set(w.pos.x, w.pos.y + OMINO_Y_OFFSET);
    if (!w.sprite) w.container.scale.x = w.facing;
  }

  /** Seeded pause duration for the next idle stop. Forum lingerers mill — they
   *  pause longer than a passer-by so the civic squares stay populated. */
  private pauseMs(w: Walker): number {
    if (w.forum) {
      return FORUM_PAUSE_MIN_MS + w.rng.float() * FORUM_PAUSE_SPAN_MS;
    }
    return PAUSE_MIN_MS + w.rng.float() * PAUSE_SPAN_MS;
  }

  // -------------------------------------------------------------------------
  // Construction / teardown
  // -------------------------------------------------------------------------

  /**
   * Is walker #`index` a forum lingerer? A deterministic function of the index
   * ALONE (every Nth index, capped at MAX_FORUM) so a walker's role is stable as
   * the crowd grows/shrinks from the tail. Requires civic anchors to exist. The
   * stride derives from FORUM_FRACTION so ~35% of the crowd lingers.
   */
  private isForumIndex(index: number): boolean {
    if (this.forumNodeIds.length === 0) return false;
    const stride = Math.max(2, Math.round(1 / FORUM_FRACTION));
    return index % stride === 0 && index / stride < MAX_FORUM;
  }

  /** Spawn walker #`index` at a resolvable random node, or null if none works. */
  private spawn(index: number): Walker | null {
    const resolve = this.resolve;
    if (!resolve || this.nodeIds.length === 0) return null;

    const rng = rngFromString(`ambient:${index}`);
    const forum = this.isForumIndex(index);
    // Find a node we can actually place on (bounded tries). Forum lingerers try
    // to start AT a civic anchor so they begin life near the square; everyone
    // else (and a forum walker that can't resolve an anchor) starts anywhere.
    let nodeId: string | null = null;
    let pos: IsoPoint | null = null;
    if (forum) {
      for (let i = 0; i < 6; i++) {
        const candidate = rng.pick(this.forumNodeIds);
        const p = candidate ? resolve(candidate) : null;
        if (candidate && p) {
          nodeId = candidate;
          pos = p;
          break;
        }
      }
    }
    for (let i = 0; !nodeId && i < 8; i++) {
      const candidate = rng.pick(this.nodeIds);
      const p = candidate ? resolve(candidate) : null;
      if (candidate && p) {
        nodeId = candidate;
        pos = p;
        break;
      }
    }
    if (!nodeId || !pos) return null;

    const type = AMBIENT_TYPES[index % AMBIENT_TYPES.length];
    // Stagger initial strolls so the crowd doesn't move in unison.
    return this.buildWalkerAt(type, nodeId, pos, rng, {
      forum,
      bobPhase: index % BOB_OFFSETS.length,
      remainingMs: 200 + index * 120,
    });
  }

  /**
   * Build a walker of `type` standing at `nodeId` (resolved `pos`), wiring up its
   * PIXI container/figure and seeding all per-walker variation from `rng`. The
   * shared construction core for both `spawn` (random-node decorative crowd) and
   * `adopt` (re-inserting a claimed omino at a snapped node), so the two never
   * drift in PIXI setup / cleanup discipline.
   *
   * The walker starts IDLE so its next update() tick picks a fresh roaming target
   * — i.e. it resumes free roaming from `nodeId`. All randomness flows through the
   * passed `rng` (deterministic), never Math.random/Date.now.
   */
  private buildWalkerAt(
    type: CitizenType,
    nodeId: string,
    pos: IsoPoint,
    rng: Rng,
    opts: { forum: boolean; bobPhase: number; remainingMs: number },
  ): Walker {
    // Subtle per-citizen tunic variation, kept on-palette (same trick as agents).
    const tunic = shadeColor(defaultTunic(type), 0.9 + rng.float() * 0.2);

    const container = new Container();
    container.position.set(pos.x, pos.y + OMINO_Y_OFFSET);
    container.alpha = AMBIENT_ALPHA;
    container.visible = this.visible;
    // Decoration: never interactive — clicks fall through to the background so
    // tapping a citizen never "selects" anything (they aren't inspectable data).
    container.eventMode = "none";

    const base = new Graphics();
    base.scale.set(FIGURE_SCALE);
    container.addChild(base);

    // A6 — the anonymous "citizen" walks with a real UH sprite when the full
    // frame table resolved; every role-typed walker (and the whole crowd when
    // the bank is absent) stays on the procedural drawCitizen path.
    let sprite: Sprite | null = null;
    let spriteSex: "m" | "f" = "m";
    if (type === "citizen" && this.walkFrames) {
      spriteSex = rng.bool(0.5) ? "m" : "f";
      sprite = new Sprite(this.walkFrames[spriteSex][2][0]); // south, frame 0
      // Feet registration comes from the manifest (via the table), so future
      // walker art with a non-default anchor stays aligned automatically.
      const [ax, ay] = this.walkFrames.anchor;
      sprite.anchor.set(ax, ay);
      sprite.scale.set(WALK_SPRITE_SCALE);
      container.addChild(sprite);
    }
    this.root.addChild(container);

    const phase = rng.float() * Math.PI * 2;

    // P5.2 — per-walker lane offset from seeded rng (deterministic).
    const laneOff = laneOffset(`ambient:${nodeId}:${type}`);
    // P5.2 — stable walker id for slot allocation.
    const wid = `amb:${nodeId}:${type}:${rng.float().toString(36).slice(2, 6)}`;
    const w: Walker = {
      rng,
      type,
      tunic,
      container,
      base,
      sprite,
      spriteSex,
      dirIdx: 2, // south (toward the camera) until the first stroll
      pos: { x: pos.x, y: pos.y },
      nodeId,
      phase,
      bobPhase: opts.bobPhase,
      facing: 1,
      forum: opts.forum,
      // P5.2 — idle variant: forum lingerers prefer sit (~50%), others get look-around (~25%)
      wid,
      idleVariant: opts.forum
        ? (rng.float() < 0.5 ? "sit" : "none")
        : (rng.float() < 0.25 ? "lookAround" : "none"),
      laneOff,
      state: { kind: "idle", remainingMs: opts.remainingMs },
    };
    // Sprite walkers never draw into `base` (it stays an empty Graphics).
    if (!sprite) {
      drawCitizen(base, type, { moving: false, phase, actionPhase: 0, tunic });
    }
    return w;
  }

  private destroyWalker(w: Walker): void {
    // P5.2 — release entry slot + sweep on walker removal.
    this.slotAllocator.release(w.nodeId, w.wid);
    this.slotAllocator.sweep(w.wid);
    // Detach from `this.root` BEFORE destroying so the parent never retains a
    // dead child ref the next-frame render would touch. Guard on `.destroyed`
    // so a double-call (clear() then a layer destroy({children:true})) is safe.
    if (!(w.container as Container & { destroyed?: boolean }).destroyed) {
      w.container.removeFromParent();
      w.container.destroy({ children: true });
    }
  }

  clear(): void {
    // Idempotent: destroyWalker guards on `.destroyed` and emptying the array
    // makes a second call a no-op. Each walker is detached before destroy so the
    // layer is left with no live children to double-destroy.
    for (const w of this.walkers) this.destroyWalker(w);
    this.walkers = [];
    // Polis-P3 — a cleared layer has NO live walkers and NO live agents, so the
    // claim offset must reset too. Without this, a `release` whose matching
    // `adopt` never arrives (an agent that vanished — see the P4 contract below)
    // would leak claimedCount permanently: every future setCount(n) would
    // undershoot by the leak, shrinking the crowd forever. A full teardown
    // (setWorld with an empty graph, or a city reload) heals any such drift here.
    //
    // P4 CONTRACT: AgentLayer MUST pair every `release` (claimedCount++) with an
    // eventual `adopt` (claimedCount--) — OR a full-teardown reset via clear() —
    // when the claimed agent ends. This AmbientLayer-level reset ONLY covers full
    // teardown, NOT a per-agent vanish; per-agent release↔adopt pairing is P4's
    // responsibility, enforced by P4's own tests.
    this.claimedCount = 0;
    // P5.2 — clear entry slots on scene teardown.
    this.slotAllocator.clear();
  }
}
