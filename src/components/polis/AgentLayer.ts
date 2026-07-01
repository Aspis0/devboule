// AgentLayer — renders agents as geometric "omini" with STEPPED, state-based
// poses on the map, and WALKS them along the streets between buildings (the
// "AgentMover" from the design doc) instead of teleporting.
//
// HONESTY RULE: only agents whose `currentFileId` resolves to a real building
// are placed. Agents with `currentFileId === null` (or pointing at a file that
// is not a rendered building), and the divine `augur`, are NOT drawn here —
// PolisView surfaces them in a roster panel instead. We never invent a position,
// and an agent only ever walks between REAL buildings along REAL roads.
//
// Each placed agent gets:
//   - a glow ellipse UNDER the building (the "something is happening here" cue),
//     pulsing in DISCRETE stepped levels (no smooth sine); the glow FOLLOWS the
//     omino while it walks,
//   - a geometric omino (head + body + legs, colored by agent TYPE), LOD-gated,
//   - STEPPED BOBBING: a 1–2px vertical micro-bounce toggled among 3 discrete
//     y-offsets on the 30fps step clock (pre-rendered-sprite cadence),
//   - a STATE-BASED POSE driven by the agent's REAL status: working → a hammer
//     swinging between 2 stepped poses; reviewing → a magnifier/eye glint
//     toggling; walking → two alternating leg poses; idle → still.
//
// MOVEMENT (AgentMover): agents are tracked by agentId across applyDecisions()
// calls — the possession-driven entry point. The PURE PossessionController owns
// who is on the map (claim-from-crowd vs spawn-fresh, building changes, vanish)
// and the augur/off-map gating; AgentLayer only APPLIES the decisions it emits
// against PIXI. When the controller emits a `walk` (an agent's currentFileId
// changed) we compute a road route and set the omino WALKING along the polyline
// (slow, Zeus-like, a few tiles/second). On arrival it returns to its real status
// pose. If no road route exists we fall back to a fade-out/reposition/fade-in
// teleport so the figure never slides glitchily through buildings. Fresh agents
// fade in; released agents fade out and destroy.
//
// PERFORMANCE: all pose variants are pre-built ONCE per agent; the step clock
// and the per-frame update only flip child visibility and nudge the omino's
// position/scale/alpha — no geometry is rebuilt and no Graphics is allocated per
// frame. Routes are computed ONLY when an agent actually changes building.

import { Container, Graphics, Rectangle } from "pixi.js";
import type { Agent, AgentStatus, AgentType } from "../../types/city";
import { type IsoPoint } from "./iso";
import { agentColor } from "./palette";
import { steppedPulse } from "./effects";
import { hashString } from "./rng";
import {
  drawCitizen,
  defaultTunic,
  shadeColor,
  type CitizenType,
} from "./kitcd/people";
// Type-only import (erased at runtime, so NO import cycle with possession.ts,
// which imports figureForAgent/figureForRole as VALUES from this module): the
// AgentLayer APPLIES the PURE PossessionController's decisions against PIXI.
import type { PossessionDecision } from "./possession";

const AGENT_SIZE = 9;

// The kitcd citizen figures are authored ~23px tall (head at y≈-19, feet at
// y=0); the old omino was ~12px. Scale the whole figure down so it reads at the
// same on-map size, and lift it so its feet sit on the omino anchor.
export const FIGURE_SCALE = 0.55;

// Polis-P4 — a SUBAGENT omino is the SCALED-DOWN figure of its role (a fan-out
// helper drawn next to its parent coder). This phase (P2) only defines the
// helper + the scale so P4 can spawn the smaller figure; no subagent is drawn
// yet. The scale is a pure transform on the SAME procedural figure (no new art).
export const SUBAGENT_FIGURE_SCALE = 0.65;

/** Effective node scale for a subagent omino: the base figure scale reduced by
 *  the subagent factor. Pure helper for P4 — a subagent's Graphics node is set
 *  to this so it reads as a smaller version of the same role figure. */
export function subagentFigureScale(): number {
  return FIGURE_SCALE * SUBAGENT_FIGURE_SCALE;
}
// Figure feet are at y=0 in figure space; scaled feet sit at the anchor, so no
// extra y shift is needed beyond OMINO_Y_OFFSET — the figure draws upward.

// Map a Polis agent `type` slug -> a kitcd citizen figure (kitcd/people.ts):
//   coder        -> builder  (Tekton, swings a hammer)   [1:1 Greek label]
//   orchestrator -> noble    (Eupatrides, himation cloak + staff; the authority)
//   verifier     -> citizen  (Polites) + a magnifier overlay (drawn below)
//   augur        -> never drawn (skipped off-map in setAgents)
//   anything else-> citizen  (plain Polites)
//
// ROLE UNTANGLE (2026-07): "orchestrator" is a FIRST-CLASS stored role again and
// the `type` slug is a pass-through of it (polis/scanner.rs `derived_agent_type`
// no longer promotes a coder that fans out to subagents). Only a real stored
// role:"orchestrator" session renders the noble; this figure map stays type-driven.
//
// NOTE: `firefighter` is deliberately NOT produced here. It is reserved for the
// Censor presence feed (Polis-P5), which is NOT an Agent and drives its figure
// through a separate non-agent path — so no agent `type` ever maps to it.
function figureForType(type: AgentType): CitizenType {
  switch (type) {
    case "coder":
      return "builder";
    case "orchestrator":
      return "noble";
    case "verifier":
      return "citizen";
    default:
      return "citizen";
  }
}

/**
 * Polis-P2 — pick the kit figure for a whole {@link Agent}, honouring the
 * mini-coder distinction the plain `type` map cannot express:
 *
 *   - `parentAgentId` SET ⇒ **watercarrier** (a mini-coder spawned by a parent
 *     coder). This takes PRECEDENCE over `type`, regardless of the underlying
 *     type slug.
 *   - otherwise fall back to the type map ({@link figureForType}): coder→builder,
 *     orchestrator→noble, verifier→citizen, anything else→citizen.
 *
 * Agent rendering routes through here; {@link figureForType} stays for any
 * type-only callers.
 */
export function figureForAgent(
  agent: Pick<Agent, "type" | "parentAgentId">,
): CitizenType {
  if (agent.parentAgentId) return "watercarrier";
  return figureForType(agent.type);
}

/**
 * Polis-P2 — pick the kit figure for a SUBAGENT by its role slug (used by P4 to
 * render each subagent as the scaled-down figure of its OWN role):
 * coder→builder, verifier→citizen, anything else→citizen. A subagent of a coder
 * is itself doing coder work, so it draws as a builder; the SCALE
 * ({@link subagentFigureScale}) is what marks it as a subordinate, not the figure.
 */
export function figureForRole(role: string): CitizenType {
  switch (role) {
    case "coder":
      return "builder";
    case "verifier":
      return "citizen";
    default:
      return "citizen";
  }
}

// A subtly-varied per-citizen tunic so a crowd of the same role isn't uniform.
// Stays on-palette: it only nudges the figure's OWN default tunic ±~10% via the
// figure's own shade transform, keyed deterministically off the agentId hash.
// Exported so the deterministic mapping can be unit-tested and re-derived when a
// figure changes (same figure + seed ⇒ identical tunic).
export function tunicForAgent(figure: CitizenType, seed: number): number {
  const base = defaultTunic(figure);
  // factor in [0.9, 1.1) from the seed (no Math.random).
  const factor = 0.9 + ((seed >>> 8) % 200) / 1000;
  return shadeColor(base, factor);
}

/**
 * Polis-P2 — the role-specific `actionPhase` fed to {@link drawCitizen} for a
 * given figure + status + step frame. Pure + deterministic (no Math.random):
 *
 *   - `builder` (coder): swings a hammer while working/running; idle = rest (0).
 *   - `firefighter`: throws a water arc ONLY when `extinguishing` is explicitly
 *     true. Polis-P5's Censor presence feed owns that flag; an idle or walking
 *     firefighter (the default, `extinguishing` false/undefined) returns 0 so it
 *     reads as a plain bucket-carrier and never spuriously throws water.
 *   - anything else: 0 (no role action).
 */
export function actionPhaseFor(
  figure: CitizenType,
  status: string,
  frame: number,
  extinguishing?: boolean,
): number {
  if (figure === "builder") {
    const busy = status === "working" || status === "running";
    return busy ? frame * 0.5 : 0; // source used this.t*5; steady swing
  }
  if (figure === "firefighter") {
    // GATED on P5's explicit flag — emulate the source's `this.t` seconds only
    // while actively extinguishing; otherwise no water arc.
    return extinguishing ? frame * (1 / 30) : 0;
  }
  return 0;
}

// Discrete bob offsets (px), cycled on the step clock.
const BOB_OFFSETS = [0, -1, -2, -1] as const;
// Discrete glow alpha levels (stepped pulse, not a sine).
const GLOW_LEVELS = [0.22, 0.34, 0.45, 0.34] as const;

// Real-agent marker: a small gold arrow hovering above the head. This is the
// visual distinction from the DECORATIVE ambient crowd (AmbientLayer), which
// carries no marker — only entries in `city.agents` (real sessions) get one.
const MARKER_Y = -17;
const MARKER_HOVER = [0, -1, -1.5, -1] as const;

// Walking speed in ISO pixels per second. Deliberately slow + visible
// ("Zeus-like"): one iso tile is ~TILE_W/2..TILE_H/2 across, so this reads as a
// few tiles/second. Movement is driven from the same 30fps step cadence but
// position lerps SMOOTHLY along each segment (smoother travel than a hard step,
// per the doc's allowance) using the real elapsed ms.
const WALK_SPEED = 70; // px/s
// Fade-teleport (no-road fallback) durations, in ms.
const FADE_OUT_MS = 200;
const FADE_IN_MS = 200;
// New-agent / arrival fade-in duration, in ms. Also drives the subagent fade.
const APPEAR_MS = 200;
// The omino's resting vertical offset above its glow anchor.
const OMINO_Y_OFFSET = -4;
// Polis-P4 — subagent omini read as subordinate EXTRAS: a touch more transparent
// than the full-alpha real agents so the arrow-marked parents always pop.
const SUB_ALPHA = 0.85;

type MoveMode =
  // Standing at `pos`; normal in-place pose + bob.
  | { kind: "idle" }
  // Walking along `route` from `route[seg]`→`route[seg+1]`, `t` in [0,1).
  | { kind: "walk"; route: IsoPoint[]; seg: number; t: number }
  // Fade-teleport: phase out at old pos, jump, phase in at `target`.
  | { kind: "fadeOut"; elapsed: number; target: IsoPoint }
  | { kind: "fadeIn"; elapsed: number }
  // Appearance fade for a brand-new agent.
  | { kind: "appear"; elapsed: number };

interface PlacedAgent {
  agent: Agent;
  color: number;
  /** Current ISO position of the omino's anchor (glow sits here, omino above). */
  pos: IsoPoint;
  /** The building the agent is currently AT (its currentFileId at last update). */
  fileId: string;
  glow: Graphics;
  /** Container at pos that we bob vertically + translate while walking. */
  omino: Container;
  /** The single citizen-figure Graphics; CLEARED + REDRAWN each step (the kitcd
   *  figures animate limbs by clear+redraw, exactly like the source `_draw`).
   *  Gated to LOD-visible agents in {@link step}. */
  base: Graphics;
  /** Gold "this is a REAL agent" arrow hovering above the head — the visual
   *  distinction from decorative ambient citizens (AmbientLayer has none). NULL
   *  for an EXTERNAL engine omino (Polis-P5 Censor firefighter): it is not a
   *  session, so it carries no real-agent marker. */
  marker: Graphics | null;
  /** Which kitcd figure this role draws (coder→builder, orchestrator→noble, …). */
  figure: CitizenType;
  /** Polis-P5 — when true the figure is PINNED: pose/status changes do NOT
   *  re-derive it from an Agent (the Censor firefighter is not an agent, so
   *  figureForAgent would never yield "firefighter"). Default false/undefined for
   *  real agents (their figure tracks parentAgentId/type as before). */
  pinnedFigure?: boolean;
  /** Polis-P5 OWNS THIS. When true and {@link figure} is `firefighter`, the
   *  water-arc actionPhase is pumped (the Censor "actively reviewing" tell).
   *  Default false/undefined: an idle/walking firefighter shows NO water — see
   *  {@link actionPhaseFor}. No real agent type maps to firefighter today; P5's
   *  Censor presence feed sets this flag when it renders a reviewing firefighter. */
  extinguishing?: boolean;
  /** Per-citizen tunic colour (deterministic from {@link seed}). */
  tunic: number;
  /** The agentId hash used to derive {@link tunic}. Stored so the tunic can be
   *  re-derived deterministically if {@link figure} later changes. */
  seed: number;
  /** The EFFECTIVE status driving the figure: "walking" while travelling even
   *  though agent.status differs; the real status otherwise. */
  effectiveStatus: AgentStatus;
  /** Walk-cycle phase in radians; advances while the figure is moving (the
   *  source's `this.walkPhase`). Drives leg/arm swing deterministically. */
  walkPhase: number;
  /** Per-agent phase offset so the crowd doesn't move in perfect unison. */
  phase: number;
  /** Movement state machine. */
  move: MoveMode;
  /** Horizontal facing (+1 right, -1 left); flips with travel direction. */
  facing: number;
}

/**
 * Polis-P4 — a SUBAGENT omino: a small, scaled-down figure of its role that mills
 * near its parent's building. SCENERY-adjacent EXTRAS (no glow, no clickable
 * inspect, no real-agent arrow), but DATA-DERIVED from the parent's
 * `subagents:[{role,count}]` — so they are spawned DIRECTLY here (option (b)), NOT
 * claimed from the ambient crowd (zero claimedCount churn). They fade in on spawn
 * and fade out on removal.
 */
interface PlacedSub {
  /** Container at `pos`; bobbed vertically each step (scaled figure inside). */
  omino: Container;
  /** The scaled figure Graphics, cleared + redrawn each visible step. */
  base: Graphics;
  figure: CitizenType;
  tunic: number;
  pos: IsoPoint;
  /** Walk-cycle phase (idle subagents barely move; kept for a faint sway). */
  walkPhase: number;
  phase: number;
  /** Fade-in/out envelope: target alpha + elapsed (ms). null once settled at 1. */
  fade: { dir: "in" | "out"; elapsed: number } | null;
}

export class AgentLayer {
  private root: Container;
  // Tracked by agentId so identity survives across reconcile (applyDecisions)
  // calls — the key to walking instead of teleporting.
  private placed = new Map<string, PlacedAgent>();
  // Polis-P4 — subagent omini, keyed by their stable subId (deterministic, from
  // the PossessionController). Separate from `placed` so they never collide with
  // real agentIds and are torn down independently.
  private subs = new Map<string, PlacedSub>();
  // Polis-P5 — EXTERNAL, engine-driven omini keyed by a stable string id (e.g.
  // the Censor firefighter `censor:<projectId>`). Tracked SEPARATELY from `placed`
  // so they NEVER appear in the agent roster / `placedCount`, never collide with
  // real agentIds, and are torn down independently. They reuse the full walk /
  // pose / glow machinery (a PlacedAgent under the hood) but draw a PINNED figure
  // (not derived from any Agent), carry NO gold real-agent marker (they are not a
  // session), and never fire the onSelectAgent callback. The Censor firefighter is
  // the only producer today; its `extinguishing` flag gates the water-arc tell.
  private externals = new Map<string, PlacedAgent>();
  private ominoVisible = true;
  private onSelectAgent?: (agent: Agent | null) => void;

  constructor(root: Container, onSelectAgent?: (agent: Agent | null) => void) {
    this.root = root;
    this.onSelectAgent = onSelectAgent;
  }

  /** Number of agents actually drawn on the map. */
  get placedCount(): number {
    return this.placed.size;
  }

  /** Number of subagent omini currently drawn (Polis-P4). */
  get subagentCount(): number {
    return this.subs.size;
  }

  /** Current ISO anchor of a placed agent's omino, or null if not placed. The
   *  PossessionController reads this to adopt a vanished claimed walker back into
   *  the crowd at exactly where its omino stood. */
  agentPos(agentId: string): IsoPoint | null {
    const p = this.placed.get(agentId);
    return p ? { x: p.pos.x, y: p.pos.y } : null;
  }

  /**
   * Polis-P4 — APPLY the PURE PossessionController's decisions against PIXI. The
   * controller already decided claim/spawn-fresh/walk/release and drove the
   * AmbientLayer release/adopt accounting; this method only performs the
   * corresponding scene-graph mutations, reusing the existing walk machine.
   *
   * @param findRoute optional road route between two graph nodes / buildings
   *                  (null → fade-teleport). The decision carries the resolved
   *                  destination iso, so no fileId→iso resolver is needed here.
   */
  applyDecisions(
    decisions: readonly PossessionDecision[],
    findRoute?: (fromFileId: string, toFileId: string) => IsoPoint[] | null,
  ): void {
    for (const d of decisions) {
      switch (d.kind) {
        case "createClaimed": {
          // Skip a duplicate create (defensive: the controller never double-claims,
          // but an apply must be idempotent against its own tracked set).
          if (this.placed.has(d.agentId)) break;
          // ACCEPTED EDGE (F6): the controller already drove env.release before
          // emitting this. If createAgent throws here (a catastrophic PIXI failure,
          // e.g. GL context lost) the claimed walker is invisible — but claimedCount
          // still balances on the agent's eventual vanish (adopt). We do NOT wrap
          // this in try/catch: a lost GL context is unrecoverable and would tear the
          // whole canvas down anyway, so the added complexity buys nothing.
          const p = this.createAgent(d.agent, d.startPos, d.targetFileId, {
            initialMove: "idle",
          });
          this.placed.set(d.agentId, p);
          // Walk it out of the crowd toward its building (or fade-teleport if no
          // honest road route from the handoff node exists).
          const route = findRoute ? findRoute(d.startNodeId, d.targetFileId) : null;
          if (route && route.length >= 2) {
            this.beginWalk(p, route, d.targetIso);
          } else {
            this.beginFadeTeleport(p, d.targetIso);
          }
          break;
        }
        case "createFresh": {
          if (this.placed.has(d.agentId)) break;
          this.placed.set(
            d.agentId,
            this.createAgent(d.agent, d.targetIso, d.targetFileId),
          );
          break;
        }
        case "refresh": {
          const p = this.placed.get(d.agentId);
          if (!p) break;
          p.agent = d.agent;
          // Only refresh the pose when standing — a mid-travel omino keeps moving.
          if (p.move.kind === "idle") this.refreshIdlePose(p);
          break;
        }
        case "walk": {
          const p = this.placed.get(d.agentId);
          if (!p) break;
          const prevFileId = p.fileId;
          p.agent = d.agent;
          p.fileId = d.targetFileId;
          const route = findRoute ? findRoute(prevFileId, d.targetFileId) : null;
          if (route && route.length >= 2) {
            this.beginWalk(p, route, d.targetIso);
          } else {
            this.beginFadeTeleport(p, d.targetIso);
          }
          break;
        }
        case "release": {
          const p = this.placed.get(d.agentId);
          if (!p) break;
          this.destroyAgent(p);
          this.placed.delete(d.agentId);
          break;
        }
        case "spawnSub": {
          const existing = this.subs.get(d.subId);
          if (existing) {
            // The sub is still tracked. If it is mid FADE-OUT (a count 3→2→3 within
            // the fade window re-spawned it before update() destroyed it), REVIVE it:
            // interrupt the fade-out and restart a fade-IN from its current alpha so
            // it stays visible. Without this the in-flight fade-out would destroy it
            // on completion while the controller believes it placed → permanently
            // invisible + subagentCount divergence. Re-anchor to the new ring pos so
            // a revive after a parent move lands correctly. If it is NOT fading out
            // (already present + settled/fading-in) the spawn is a redundant no-op.
            if (existing.fade?.dir === "out") {
              existing.pos = { x: d.pos.x, y: d.pos.y };
              existing.omino.position.set(d.pos.x, d.pos.y + OMINO_Y_OFFSET);
              // Resume the fade-IN from the CURRENT alpha so there is no visual jump:
              // elapsed is back-solved from the alpha already on screen.
              const cur = Math.max(0, Math.min(1, existing.omino.alpha / SUB_ALPHA));
              existing.fade = { dir: "in", elapsed: cur * APPEAR_MS };
            }
            break;
          }
          this.subs.set(d.subId, this.createSubagent(d.subId, d.figure, d.pos));
          break;
        }
        case "moveSub": {
          const s = this.subs.get(d.subId);
          if (!s) break;
          // Snap to the new ring position (parent changed building). The step clock
          // applies the bob from `pos`, so updating it here re-anchors the omino.
          s.pos = { x: d.pos.x, y: d.pos.y };
          s.omino.position.set(d.pos.x, d.pos.y + OMINO_Y_OFFSET);
          break;
        }
        case "removeSub": {
          const s = this.subs.get(d.subId);
          if (!s) break;
          // Fade out, then destroy on the next settled frame. Simpler + leak-free:
          // mark it fading-out; update() destroys it when the fade completes.
          // DO NOT re-arm a fade-out already in flight: if diffs arrive faster than
          // APPEAR_MS, resetting elapsed=0 on each removeSub would perpetually
          // restart the fade so it never completes → the sub never disappears. Only
          // arm a FRESH fade-out when not already fading out.
          if (s.fade?.dir !== "out") s.fade = { dir: "out", elapsed: 0 };
          break;
        }
      }
    }
  }

  // -------------------------------------------------------------------------
  // Polis-P5 — EXTERNAL engine omini (the Censor firefighter). Driven directly
  // by the renderer applying CensorPresence decisions, NOT through the agent diff.
  // Keyed by a stable string id (`censor:<projectId>`). They reuse the full walk /
  // pose / glow machine but draw a pinned firefighter figure, carry no marker, and
  // are never inspectable / never in the roster (`placedCount` excludes them).
  // -------------------------------------------------------------------------

  /** Number of external engine omini currently drawn (Polis-P5; for tests). */
  get externalCount(): number {
    return this.externals.size;
  }

  /** Current ISO anchor of an external omino, or null if absent. The Censor
   *  presence reads this to adopt a released firefighter back into the crowd at
   *  exactly where its omino stood (mirrors {@link agentPos}). */
  externalPos(id: string): IsoPoint | null {
    const p = this.externals.get(id);
    return p ? { x: p.pos.x, y: p.pos.y } : null;
  }

  /** A minimal synthetic Agent backing an external omino's PlacedAgent struct. It
   *  is NEVER exposed (no marker, no inspect, never in `placed`/the roster) — it
   *  only feeds the shared machine's color/seed/status. The figure is PINNED so
   *  this synthetic `type` never drives the drawn figure. */
  private syntheticExternalAgent(id: string): Agent {
    return {
      agentId: id,
      type: "verifier",
      status: "reviewing",
      currentFileId: null,
      currentTask: null,
      // Firefighter-red glow (the kit firefighter tunic tone) so the engine omino
      // reads distinct from the gold-marked real agents.
      color: "#b23a30",
    };
  }

  /** Create an external firefighter omino starting AT `startPos` (a crowd walker
   *  just released there) and WALK it toward the reviewed building. No appear-fade
   *  (it steps straight out of the crowd). Idempotent: a duplicate id is ignored. */
  createExternalClaimed(
    id: string,
    figure: CitizenType,
    startPos: IsoPoint,
    startNodeId: string,
    targetFileId: string,
    targetIso: IsoPoint,
    findRoute?: (fromFileId: string, toFileId: string) => IsoPoint[] | null,
  ): void {
    if (this.externals.has(id)) return;
    const p = this.createAgent(this.syntheticExternalAgent(id), startPos, targetFileId, {
      initialMove: "idle",
      figure,
      marker: false,
    });
    this.externals.set(id, p);
    const route = findRoute ? findRoute(startNodeId, targetFileId) : null;
    if (route && route.length >= 2) this.beginWalk(p, route, targetIso);
    else this.beginFadeTeleport(p, targetIso);
  }

  /** Create an external firefighter omino FRESH at its target building (appear-
   *  fade). Idempotent: a duplicate id is ignored. */
  createExternalFresh(
    id: string,
    figure: CitizenType,
    targetFileId: string,
    targetIso: IsoPoint,
  ): void {
    if (this.externals.has(id)) return;
    const p = this.createAgent(this.syntheticExternalAgent(id), targetIso, targetFileId, {
      figure,
      marker: false,
    });
    this.externals.set(id, p);
  }

  /** Walk an existing external omino to a different reviewed building (no re-claim;
   *  fade-teleport when no road route). No-op if the id isn't placed. */
  walkExternal(
    id: string,
    targetFileId: string,
    targetIso: IsoPoint,
    findRoute?: (fromFileId: string, toFileId: string) => IsoPoint[] | null,
  ): void {
    const p = this.externals.get(id);
    if (!p) return;
    const prevFileId = p.fileId;
    p.fileId = targetFileId;
    const route = findRoute ? findRoute(prevFileId, targetFileId) : null;
    if (route && route.length >= 2) this.beginWalk(p, route, targetIso);
    else this.beginFadeTeleport(p, targetIso);
  }

  /** Toggle the water-arc tell (the P2 `extinguishing` gate) on an external
   *  firefighter omino. No-op if the id isn't placed. */
  setExternalExtinguishing(id: string, on: boolean): void {
    const p = this.externals.get(id);
    if (p) p.extinguishing = on;
  }

  /** Destroy an external omino (released back to roaming). No-op if absent. */
  destroyExternal(id: string): void {
    const p = this.externals.get(id);
    if (!p) return;
    this.destroyAgent(p);
    this.externals.delete(id);
  }

  setLodVisible(visible: boolean): void {
    this.ominoVisible = visible;
    for (const p of this.placed.values()) p.omino.visible = visible;
    for (const p of this.externals.values()) p.omino.visible = visible;
    for (const s of this.subs.values()) s.omino.visible = visible;
  }

  /**
   * Advance the stepped pose / bob / glow for ONE placed omino (real agent OR an
   * external engine omino). Extracted so the Censor firefighter (Polis-P5) reuses
   * the exact same retro stepped cadence as real agents. No allocation, no rebuild.
   */
  private stepPlaced(p: PlacedAgent, frame: number): void {
    // Glow pulse runs at any zoom but is stepped. EXCEPTION: while a fade
    // (appear / fade-teleport) owns the glow's alpha, leave it alone so the
    // stepped pulse doesn't clobber the fade. Walking + idle pulse normally;
    // while walking the glow tracks the omino's position (set in update()).
    if (p.move.kind === "idle" || p.move.kind === "walk") {
      p.glow.alpha = steppedPulse(frame, GLOW_LEVELS, 2);
    }

    // Omino animation only when it's actually shown (LOD-gated == visible).
    if (!p.omino.visible) return;

    // Stepped vertical bob (suppressed mid-fade-teleport reposition; harmless
    // during walk — it rides on top of the travel translation).
    const bob = BOB_OFFSETS[(Math.floor(frame / 2) + p.phase) % BOB_OFFSETS.length];
    p.omino.position.y = p.pos.y + OMINO_Y_OFFSET + bob;

    // Hover the real-agent marker independently for a subtle "floating" cue.
    // EXTERNAL omini (Censor firefighter) carry no marker — skip.
    if (p.marker) {
      p.marker.position.y =
        MARKER_Y + MARKER_HOVER[(Math.floor(frame / 3) + p.phase) % MARKER_HOVER.length];
    }

    // ---- redraw the kitcd citizen figure for this frame ----
    // The figure animates limbs by clear+redraw (like the source `_draw`).
    // `status` is read as a plain string (AgentStatus is an open union, so we
    // compare without literal-narrowing the chain).
    const status: string = p.effectiveStatus;
    // "moving" = travelling, or a real walking/running status. While moving we
    // advance the deterministic walk phase on the step clock so legs/arms swing.
    const moving =
      p.move.kind === "walk" || status === "walking" || status === "running";
    if (moving) p.walkPhase += 0.6; // ~ source's walkPhase += dt*9 at 30 Hz

    // status -> action phase (role-specific). The firefighter water arc is
    // GATED on p.extinguishing (Polis-P5 owns it); see actionPhaseFor.
    const actionPhase = actionPhaseFor(p.figure, status, frame, p.extinguishing);

    drawCitizen(p.base, p.figure, {
      moving,
      phase: p.walkPhase + p.phase,
      actionPhase,
      tunic: p.tunic,
    });

    // verifier (plain citizen) shows a magnifier while reviewing/surveying —
    // the kitcd kit has no inspector figure, so overlay one on the citizen.
    if (
      p.figure === "citizen" &&
      (status === "reviewing" || status === "surveying")
    ) {
      const glint = (Math.floor(frame / 3) & 1) === 0;
      p.base.circle(5.5, -12, 2.4).stroke({ width: 1.4, color: 0x2a2a2a });
      p.base
        .moveTo(7.2, -10.3)
        .lineTo(9.2, -8.3)
        .stroke({ width: 1.4, color: 0x2a2a2a });
      p.base
        .circle(glint ? 4.7 : 6.1, -12.7, 0.9)
        .fill({ color: 0xffffff, alpha: 0.9 });
    }
  }

  /**
   * Advance one STEP (called from the shared 30fps clock). Handles the stepped
   * pose / bob / glow pulse. The actual WALKING translation is integrated from
   * real elapsed ms in {@link update} for smooth travel; this method keeps the
   * retro stepped cadence for poses and the glow.
   *
   * No allocation, no geometry rebuild.
   *
   * Polis-P6 PERF LOCK: this method (and {@link update}) MUST NEVER call findRoute.
   * Routes are computed ONLY on a building change / claim / adopt (applyDecisions,
   * createExternal*, walkExternal) — never per frame. Walking integrates the
   * PRE-COMPUTED route polyline (advanceWalk), so no pathfinding happens here.
   */
  step(frame: number, view?: Rectangle): void {
    for (const p of this.placed.values()) this.stepPlaced(p, frame);
    // Polis-P5 — EXTERNAL engine omini (Censor firefighter) use the SAME stepped
    // pose/bob/glow machine. They carry no marker (guarded) and a pinned figure.
    for (const p of this.externals.values()) this.stepPlaced(p, frame);

    // #8 — LOD halt for the SUBAGENT crowd: when zoomed out past LOD_AGENTS the
    // subs are hidden (setLodVisible(false) cleared ominoVisible), so skip the WHOLE
    // subs loop — not just the drawCitizen. The old per-sub `!s.omino.visible`
    // continue still ran the loop and (before this) advanced walkPhase per sub each
    // step even while hidden. Bailing here mirrors AmbientLayer.step's `!this.visible`
    // early return: zero per-step work for the sub crowd while LOD-hidden.
    if (!this.ominoVisible) return;

    // ---- Subagent omini: stepped bob + scaled figure redraw (LOD- + viewport-gated) ----
    for (const s of this.subs.values()) {
      // NOTE: no early `!s.omino.visible` continue here — the `!this.ominoVisible`
      // bail above already handles LOD, so while LOD-visible the only reason an
      // omino is hidden is the viewport cull below, which must be re-evaluated each
      // step so an off-screen sub that scrolls back IN is re-shown + redrawn.
      // #9 — viewport cull: skip the clear+redraw for a subagent whose pos is
      // outside the camera bounds (a margin covers the figure's own height). The
      // omino is HIDDEN while off-screen and re-shown + redrawn the step it scrolls
      // back into view — so a sub that re-enters the viewport is never left blank.
      // `view` is optional (headless tests step without one → no cull, LOD only).
      if (view && !AgentLayer.inView(s.pos, view)) {
        if (s.omino.visible) s.omino.visible = false;
        continue;
      }
      if (!s.omino.visible) s.omino.visible = true;
      const bob = BOB_OFFSETS[(Math.floor(frame / 2) + s.phase) % BOB_OFFSETS.length];
      s.omino.position.y = s.pos.y + OMINO_Y_OFFSET + bob;
      // Subagents mill in place — a faint, steady sway (not a full walk) so they
      // read as busy helpers without marching off. Deterministic phase advance.
      s.walkPhase += 0.3;
      drawCitizen(s.base, s.figure, {
        moving: true,
        phase: s.walkPhase + s.phase,
        actionPhase: actionPhaseFor(s.figure, "working", frame),
        tunic: s.tunic,
      });
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

  /** Integrate ONE placed omino's movement state machine by `deltaMs`. Extracted
   *  so the Censor firefighter (Polis-P5) reuses the exact same walk / fade-
   *  teleport / appear integration as real agents. */
  private updatePlacedMove(p: PlacedAgent, deltaMs: number): void {
    switch (p.move.kind) {
      case "walk":
        this.advanceWalk(p, p.move, deltaMs);
        break;
      case "fadeOut":
        this.advanceFadeOut(p, p.move, deltaMs);
        break;
      case "fadeIn":
        this.advanceFadeIn(p, p.move, deltaMs);
        break;
      case "appear":
        this.advanceAppear(p, p.move, deltaMs);
        break;
      case "idle":
        break;
    }
  }

  /**
   * Per-FRAME update (smooth, real elapsed ms) for the movement state machine:
   * integrates walking along the route polyline, the fade-teleport phases and
   * the appear fade. Allocation-light: only mutates position / scale.x / alpha
   * on the reused omino + glow. Pose/bob/glow-pulse stay on the stepped clock in
   * {@link step}.
   */
  update(deltaMs: number): void {
    if (deltaMs <= 0) return;
    // Polis-P6 — LOD halt: when zoomed out past LOD_AGENTS the omini are hidden, so
    // skip the per-frame MOVEMENT integration (walk lerp / fade-teleport / appear)
    // for agents AND external engine omini — nothing should animate/transform while
    // invisible. Frozen state resumes cleanly on zoom-in. NOTE: the subagent fade
    // envelopes below are NOT skipped, so a removeSub'd sub still completes its
    // fade-out and is DESTROYED even while hidden (correct teardown, no leak); it
    // is the only per-frame work that must run to keep the tracked set honest.
    if (this.ominoVisible) {
      for (const p of this.placed.values()) this.updatePlacedMove(p, deltaMs);
      // Polis-P5 — external engine omini (Censor firefighter) integrate the SAME
      // movement state machine (walk / fade-teleport / appear) as real agents.
      for (const p of this.externals.values()) this.updatePlacedMove(p, deltaMs);
    }

    // ---- Subagent fade envelopes (in on spawn, out → destroy on removal) ----
    for (const [subId, s] of this.subs) {
      if (!s.fade) continue;
      s.fade.elapsed += deltaMs;
      const t = Math.min(1, s.fade.elapsed / APPEAR_MS);
      if (s.fade.dir === "in") {
        s.omino.alpha = t * SUB_ALPHA;
        if (t >= 1) s.fade = null;
      } else {
        s.omino.alpha = (1 - t) * SUB_ALPHA;
        if (t >= 1) {
          this.destroySub(s);
          this.subs.delete(subId);
        }
      }
    }
  }

  // -------------------------------------------------------------------------
  // Movement state machine
  // -------------------------------------------------------------------------

  /** Put an agent into the WALKING state along `route`, ending at `dest`. The
   *  route already begins at (or very near) the agent's current building. We
   *  swap in the walking leg-frames and start at segment 0. */
  private beginWalk(p: PlacedAgent, route: IsoPoint[], dest: IsoPoint): void {
    // Ensure the route ends exactly at the resolved destination iso (defends
    // against tiny grid-vs-building-anchor offsets so the omino lands cleanly).
    route[route.length - 1] = { x: dest.x, y: dest.y };
    // Snap the omino's start to the route's first waypoint.
    p.pos = { x: route[0].x, y: route[0].y };
    p.move = { kind: "walk", route, seg: 0, t: 0 };
    this.setPoseStatus(p, "walking");
    this.applyTransform(p);
  }

  /** Advance walking along the polyline by `deltaMs`. Consumes segments as the
   *  agent passes their endpoints; clamps to the final waypoint on arrival and
   *  returns to the idle/real-status pose. Faces the travel direction. */
  private advanceWalk(
    p: PlacedAgent,
    m: { kind: "walk"; route: IsoPoint[]; seg: number; t: number },
    deltaMs: number,
  ): void {
    let remaining = (WALK_SPEED * deltaMs) / 1000; // px to travel this frame
    const route = m.route;
    // Guard: a route should always have >= 2 points, but clamp defensively.
    while (remaining > 0 && m.seg < route.length - 1) {
      const a = route[m.seg];
      const b = route[m.seg + 1];
      const segLen = Math.hypot(b.x - a.x, b.y - a.y) || 1e-6;
      const distLeft = segLen * (1 - m.t);
      if (remaining < distLeft) {
        // Stay within this segment.
        m.t += remaining / segLen;
        remaining = 0;
      } else {
        // Reach the segment end; carry the leftover into the next segment.
        remaining -= distLeft;
        m.seg += 1;
        m.t = 0;
      }
    }

    if (m.seg >= route.length - 1) {
      // Arrived at the final waypoint.
      const end = route[route.length - 1];
      p.pos = { x: end.x, y: end.y };
      this.faceTowards(p, route[Math.max(0, route.length - 2)], end);
      p.move = { kind: "idle" };
      this.refreshIdlePose(p);
      this.applyTransform(p);
      return;
    }

    // Mid-route: interpolate position on the current segment + face direction.
    const a = route[m.seg];
    const b = route[m.seg + 1];
    p.pos = { x: a.x + (b.x - a.x) * m.t, y: a.y + (b.y - a.y) * m.t };
    this.faceTowards(p, a, b);
    this.applyTransform(p);
  }

  /** Begin a fade-teleport: phase the omino out at its current spot, remember
   *  the destination to jump to once invisible. Used when no road route exists
   *  so the figure never slides through buildings. */
  private beginFadeTeleport(p: PlacedAgent, target: IsoPoint): void {
    p.move = { kind: "fadeOut", elapsed: 0, target: { x: target.x, y: target.y } };
  }

  private advanceFadeOut(
    p: PlacedAgent,
    m: { kind: "fadeOut"; elapsed: number; target: IsoPoint },
    deltaMs: number,
  ): void {
    m.elapsed += deltaMs;
    const t = Math.min(1, m.elapsed / FADE_OUT_MS);
    const alpha = 1 - t;
    p.omino.alpha = alpha;
    p.glow.alpha = alpha * 0.45;
    if (t >= 1) {
      // Reposition while invisible, then fade back in.
      p.pos = { x: m.target.x, y: m.target.y };
      this.refreshIdlePose(p);
      this.applyTransform(p);
      p.move = { kind: "fadeIn", elapsed: 0 };
    }
  }

  private advanceFadeIn(
    p: PlacedAgent,
    m: { kind: "fadeIn"; elapsed: number },
    deltaMs: number,
  ): void {
    m.elapsed += deltaMs;
    const t = Math.min(1, m.elapsed / FADE_IN_MS);
    p.omino.alpha = t;
    if (t >= 1) {
      p.omino.alpha = 1;
      p.move = { kind: "idle" };
    }
  }

  private advanceAppear(
    p: PlacedAgent,
    m: { kind: "appear"; elapsed: number },
    deltaMs: number,
  ): void {
    m.elapsed += deltaMs;
    const t = Math.min(1, m.elapsed / APPEAR_MS);
    p.omino.alpha = t;
    p.glow.alpha = t * 0.45;
    if (t >= 1) {
      p.omino.alpha = 1;
      p.move = { kind: "idle" };
    }
  }

  /** Set the omino + glow position from `p.pos` and apply the facing flip.
   *  Allocation-free. */
  private applyTransform(p: PlacedAgent): void {
    p.omino.position.set(p.pos.x, p.pos.y + OMINO_Y_OFFSET);
    p.omino.scale.x = p.facing;
    p.glow.position.set(p.pos.x, p.pos.y + 6);
  }

  /** Face the omino toward travel direction a→b (flip horizontally on dx), like
   *  the doc's `sprite.scale.x = dx > 0 ? 1 : -1`. A near-zero dx keeps facing. */
  private faceTowards(p: PlacedAgent, a: IsoPoint, b: IsoPoint): void {
    const dx = b.x - a.x;
    if (dx > 0.01) p.facing = 1;
    else if (dx < -0.01) p.facing = -1;
    p.omino.scale.x = p.facing;
  }

  // -------------------------------------------------------------------------
  // Pose management — the figure is redrawn from `effectiveStatus` in step();
  // these just flip which status drives the next redraw. No geometry alloc.
  // -------------------------------------------------------------------------

  /** Set the EFFECTIVE status that drives the figure's action (e.g. "walking"
   *  while travelling, back to the real status on arrival). The figure also
   *  syncs its role here in case the agent's type changed. */
  private setPoseStatus(p: PlacedAgent, status: AgentStatus): void {
    p.effectiveStatus = status;
    // Polis-P5 — a PINNED figure (Censor firefighter) is NOT re-derived from an
    // Agent: figureForAgent would never yield "firefighter", so re-deriving would
    // silently flip the engine omino to a citizen. Leave its figure + tunic intact.
    if (p.pinnedFigure) return;
    const nextFigure = figureForAgent(p.agent);
    if (nextFigure !== p.figure) {
      // The agent's figure changed (e.g. a parentAgentId appeared/disappeared,
      // flipping coder→watercarrier). The tunic was keyed to the OLD figure's
      // default tone; re-derive it from the SAME seed so it tracks the new
      // figure deterministically (same seed ⇒ same tunic).
      p.figure = nextFigure;
      p.tunic = tunicForAgent(nextFigure, p.seed);
    }
  }

  /** Restore the figure to the agent's REAL status (on arrival / standing). */
  private refreshIdlePose(p: PlacedAgent): void {
    this.setPoseStatus(p, p.agent.status);
  }

  // -------------------------------------------------------------------------
  // Construction / teardown
  // -------------------------------------------------------------------------

  /**
   * Build a placed real-agent omino. `opts.initialMove` selects how it enters:
   *   - omitted / "appear" → fades in at `pos` (a brand-new / spawn-fresh agent).
   *   - "idle" → already on-map at `pos` (a claimed crowd walker stepping out;
   *     the caller immediately walks it via {@link beginWalk}, so no fade).
   * A claimed entry starts at full alpha (it was a visible crowd walker), so we
   * don't fade it in.
   */
  private createAgent(
    agent: Agent,
    pos: IsoPoint,
    fileId: string,
    opts?: {
      initialMove?: "appear" | "idle";
      /** Polis-P5 — PIN this figure (skip Agent-derived figure). When given the
       *  omino draws this kit figure and {@link PlacedAgent.pinnedFigure} is set so
       *  pose changes never flip it. Used for the Censor firefighter. */
      figure?: CitizenType;
      /** Polis-P5 — when false, draw NO gold real-agent marker and make the omino
       *  NON-interactive (no inspect select). The Censor firefighter is an engine,
       *  not a session, so it gets neither. Default true (real agents). */
      marker?: boolean;
    },
  ): PlacedAgent {
    const claimed = opts?.initialMove === "idle";
    const pinnedFigure = opts?.figure;
    const wantMarker = opts?.marker ?? true;
    const color = agentColor(agent.type, agent.color);

    const glow = new Graphics();
    glow.position.set(pos.x, pos.y + 6);
    // Geometry built ONCE; the stepped pulse only animates glow.alpha.
    glow.ellipse(0, 0, 26, 13).fill({ color, alpha: 1 });
    // A claimed walker is already present, so its glow starts visible; a fresh
    // agent fades its glow in via the appear state.
    glow.alpha = claimed ? GLOW_LEVELS[0] : 0;
    this.root.addChild(glow);

    const omino = new Container();
    omino.position.set(pos.x, pos.y + OMINO_Y_OFFSET);
    omino.visible = this.ominoVisible;
    omino.alpha = claimed ? 1 : 0; // claimed: already visible; fresh: fade in

    // Make the citizen clickable -> opens the agent inspect popup. A generous
    // hit area around the omino body (it's a tiny ~9px figure). The tap is
    // consumed so the viewport background handler doesn't also deselect. The
    // closure reads the LIVE placed entry so a moved/refreshed agent still
    // opens the up-to-date popup. EXTERNAL engine omini (Censor firefighter) are
    // NOT sessions — they stay non-interactive so taps fall through to the
    // background (no select), exactly like the ambient crowd.
    if (wantMarker) {
      omino.eventMode = "static";
      omino.cursor = "pointer";
      omino.hitArea = new Rectangle(
        -AGENT_SIZE,
        -AGENT_SIZE * 1.2,
        AGENT_SIZE * 2,
        AGENT_SIZE * 2.4,
      );
      omino.on("pointertap", (e) => {
        e.stopPropagation();
        const entry = this.placed.get(agent.agentId);
        this.onSelectAgent?.(entry ? entry.agent : agent);
      });
    } else {
      omino.eventMode = "none";
    }

    // The single citizen-figure Graphics. The kitcd figures are authored ~23px
    // tall; scale the figure down so it reads at the old omino size. The figure
    // is drawn (cleared + redrawn) each step in {@link step} — drawn once here
    // for the appear-fade frame. Facing flips via omino.scale.x (set in
    // applyTransform), so the figure's own scale is uniform/positive.
    const base = new Graphics();
    base.scale.set(FIGURE_SCALE);
    omino.addChild(base);

    // Real-agent marker: a downward gold arrow above the head so a live agent is
    // unmistakable vs the decorative ambient crowd (which has no marker). It is a
    // symmetric triangle, so the omino's facing flip (scale.x = ±1) leaves it
    // unchanged. Drawn ONCE here; only its y hovers per-step. EXTERNAL engine
    // omini (Censor firefighter) get NO marker (null) — not a session.
    let marker: Graphics | null = null;
    if (wantMarker) {
      marker = new Graphics();
      marker.position.set(0, MARKER_Y);
      marker
        .poly([-3.5, -2.5, 3.5, -2.5, 0, 2.5])
        .fill({ color: 0xffd24a })
        .stroke({ width: 1, color: 0x4a3a10, alpha: 0.9 });
      omino.addChild(marker);
    }

    this.root.addChild(omino);

    // Deterministic phase: a per-agent radian offset from the agentId hash so
    // the crowd's walk cycles are out of step (no Math.random). The integer
    // `phase` (0..3) still drives the stepped bob cadence.
    const seed = hashString(agent.agentId);
    const phase = (agent.agentId.charCodeAt(0) || 0) % 4;
    // Polis-P5 — a PINNED figure overrides the Agent-derived one (the Censor
    // firefighter is not an agent). Otherwise derive from the agent as before.
    const figure = pinnedFigure ?? figureForAgent(agent);
    // Per-citizen tunic: start from the figure's default tone, varied subtly per
    // agent so a crowd of the same role isn't uniform. Kept on-palette by only
    // nudging the figure's own tunic.
    const tunic = tunicForAgent(figure, seed);

    const tracked: PlacedAgent = {
      agent,
      color,
      pos: { x: pos.x, y: pos.y },
      fileId,
      glow,
      omino,
      base,
      marker,
      figure,
      pinnedFigure: pinnedFigure !== undefined,
      tunic,
      seed,
      effectiveStatus: agent.status,
      walkPhase: (seed % 1000) / 1000 * Math.PI * 2,
      phase,
      // Claimed: start IDLE at the handoff pos (the caller walks it immediately);
      // fresh: run the appear-fade.
      move: claimed ? { kind: "idle" } : { kind: "appear", elapsed: 0 },
      facing: 1,
    };
    // Draw the initial frame so the appear-fade shows the figure immediately.
    drawCitizen(base, figure, {
      moving: false,
      phase: tracked.walkPhase + phase,
      actionPhase: 0,
      tunic,
    });
    return tracked;
  }

  /**
   * Polis-P4 — build a small SUBAGENT omino: the scaled-down figure of `figure`
   * at `pos`, with NO glow / NO clickable inspect / NO real-agent arrow (it is a
   * data-derived EXTRA, not a session). Starts at alpha 0 and fades in. The figure
   * uses {@link subagentFigureScale} so it reads as a subordinate of its parent.
   * Determinism: the per-figure tunic + phase are seeded from `pos` (stable for a
   * given ring slot) — no Math.random.
   */
  private createSubagent(subId: string, figure: CitizenType, pos: IsoPoint): PlacedSub {
    const omino = new Container();
    omino.position.set(pos.x, pos.y + OMINO_Y_OFFSET);
    omino.visible = this.ominoVisible;
    omino.alpha = 0; // fade in via the envelope in update()
    // Never interactive: a subagent omino isn't an inspectable session, so taps
    // fall through to the background (no select), exactly like the ambient crowd.
    omino.eventMode = "none";

    const base = new Graphics();
    base.scale.set(subagentFigureScale());
    omino.addChild(base);
    this.root.addChild(omino);

    // #6 — seed deterministic variation from the STABLE subId, NOT the position. A
    // moveSub (parent changed building) keeps the omino's identity but changes its
    // pos; a position-seeded tunic/phase would then differ from a reload that
    // re-seeds at the new pos (visual discontinuity for the SAME subagent). Seeding
    // from the subId makes appearance identity-stable across moves and reloads.
    const seed = hashString(subId);
    const tunic = tunicForAgent(figure, seed);
    const phase = (seed >>> 4) % 4;
    const walkPhase = (seed % 1000) / 1000 * Math.PI * 2;

    drawCitizen(base, figure, { moving: false, phase: walkPhase + phase, actionPhase: 0, tunic });

    return {
      omino,
      base,
      figure,
      tunic,
      pos: { x: pos.x, y: pos.y },
      walkPhase,
      phase,
      fade: { dir: "in", elapsed: 0 },
    };
  }

  private destroySub(s: PlacedSub): void {
    if (!(s.omino as Container & { destroyed?: boolean }).destroyed) {
      s.omino.removeFromParent();
      s.omino.destroy({ children: true });
    }
  }

  private destroyAgent(p: PlacedAgent): void {
    // Detach from the parent (layers.agents) BEFORE destroying so the parent
    // never retains a dead child ref that the next-frame render would touch.
    // Guard on `.destroyed` so a double-destroy (e.g. clear() then layer
    // destroy({children:true})) can never throw on an already-freed object.
    if (!(p.glow as Graphics & { destroyed?: boolean }).destroyed) {
      p.glow.removeFromParent();
      p.glow.destroy();
    }
    if (!(p.omino as Container & { destroyed?: boolean }).destroyed) {
      p.omino.removeFromParent();
      p.omino.destroy({ children: true });
    }
  }

  clear(): void {
    // Idempotent: destroyAgent guards each object on `.destroyed`, and clearing
    // the map below makes a second call a no-op loop. Each agent is detached
    // (removeFromParent) before destroy so the layer is left with no live
    // children — a subsequent layer.destroy({children:true}) has nothing to
    // double-destroy.
    for (const p of this.placed.values()) this.destroyAgent(p);
    this.placed.clear();
    // Polis-P5 — tear down external engine omini (Censor firefighter) too so a
    // city reload leaves no stray firefighter (CensorPresence.clear() pairs this).
    for (const p of this.externals.values()) this.destroyAgent(p);
    this.externals.clear();
    // Polis-P4 — tear down subagent omini too so a city reload leaves no extras.
    for (const s of this.subs.values()) this.destroySub(s);
    this.subs.clear();
  }
}
