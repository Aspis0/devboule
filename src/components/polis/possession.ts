// possession.ts — Polis-P4 PossessionController (PURE decision layer).
//
// The orchestration that makes an ACTIVATING agent "take possession" of an idle
// roaming omino and walk it to its building, plus the derivation of the small
// per-subagent omini that mill around their parent. This module is PURE and
// headless-testable: it holds no PIXI, no clock, and no Math.random / Date.now.
// All randomness is derived deterministically from stable ids + indices.
//
// ┌─────────────────────────────────────────────────────────────────────────┐
// │ RESPONSIBILITY SPLIT                                                      │
// │  - PossessionController DECIDES: claim-from-crowd vs spawn-fresh, walk on │
// │    a building change, release on vanish, and which subagent omini to      │
// │    spawn/remove. It also OWNS the claimedCount contract: it calls         │
// │    env.release / env.adopt so the AmbientLayer accounting stays balanced. │
// │  - The renderer APPLIES the returned decisions against AgentLayer (create │
// │    / walk / fade / destroy the actual PIXI omini). The controller never   │
// │    touches PIXI itself.                                                   │
// └─────────────────────────────────────────────────────────────────────────┘
//
// claimedCount CONTRACT (the P3↔P4 pairing this phase enforces):
//   - EXACTLY ONE env.adopt per successful env.release (claim-from-crowd).
//   - NEVER an env.adopt for a fallback spawn-fresh agent (no crowd figure was
//     taken, so adopting would inflate the crowd).
//   - NEVER an env.release / env.adopt for a subagent (option (b) below: subagents
//     are spawned directly as small AgentLayer omini, never from the crowd).
//   - No double-claim: a placed agent is claimed at most once; a building change
//     only walks it (no re-claim); a leaked claim on vanish is impossible because
//     the vanish path adopts iff the agent was claim-from-crowd.
//
// SUBAGENT STRATEGY — option (b), spawn-direct (NOT claim-from-crowd):
//   Subagents are EPHEMERAL and NUMEROUS (a parent can fan out and collapse many
//   times). Claiming/adopting each from the ambient crowd would churn the
//   claimedCount heavily and steal/return scenery walkers on every fan-out tick.
//   Subagents are conceptually "extras" of their parent, not members of the
//   decorative crowd, so we spawn them DIRECTLY as small AgentLayer omini near
//   the parent (fade in/out), WITHOUT touching the ambient layer at all. This
//   keeps the claimedCount contract trivially correct for subagents (zero
//   release/adopt) and avoids per-fan-out crowd thrash.

import type { Agent, AgentSubagentBrief } from "../../types/city";
import type { IsoPoint } from "./iso";
import type { CitizenType } from "./kitcd/people";
import { figureForAgent, figureForRole } from "./AgentLayer";
import { hashString } from "./rng";

/** Per-parent hard cap on derived subagent omini, to bound the figure count so a
 *  parent reporting an absurd subagent total never floods the map. Overflow is
 *  dropped (and reported via {@link PossessionResult.droppedSubagents}). */
export const MAX_SUBAGENT_OMINI_PER_PARENT = 6;

/**
 * Polis-P6 — GLOBAL hard cap on derived subagent omini ACROSS ALL parents, on top
 * of the per-parent cap. A fleet of orchestrators (each within its own per-parent
 * cap) could otherwise multiply subagent omini unbounded; this bounds the TOTAL
 * subagent population so the walker budget stays inside the "pure-data, lazy
 * PixiJS" contract.
 *
 * DROP RULE (deterministic + stable, no flicker at the boundary): subagents are
 * admitted in the SAME order they are reconciled — parents in `liveAgents` order
 * (the backend emits a stable order), and WITHIN a parent in declared
 * role/per-role-index order ({@link deriveSubagents}). Once the global budget is
 * exhausted, every further omino is DROPPED — i.e. the HIGHEST-index parents and,
 * within the parent that straddles the boundary, the highest-index slots are the
 * ones dropped. The same input therefore always drops the same omini (no random,
 * no churn): a parent that fit last diff fits this diff.
 *
 * Total-omino ceiling (documented contract): the live walker population is bounded
 * by MAX_AMBIENT (40, AmbientLayer) + live real agents + this global subagent cap
 * (24) + at most ONE Censor firefighter. Real agents are bounded by the backend's
 * own session count, not by Polis; everything Polis DERIVES (crowd, subagents,
 * firefighter) is hard-capped here and in AmbientLayer.
 */
export const MAX_SUBAGENT_OMINI_GLOBAL = 24;

/** Radius (ISO px) of the deterministic ring the subagent omini sit on around
 *  their parent building. Derived offsets only — never random, never per-frame. */
const SUBAGENT_RING_RADIUS = 22;

/**
 * The environment the PURE controller talks to. Abstracts the AmbientLayer claim
 * primitives and the position/target lookups so the controller is unit-testable
 * with a plain mock (no PIXI). The controller calls these during reconcile.
 */
export interface PossessionEnv {
  /** Resolve a building fileId to its ISO anchor, or null if not on-map. */
  resolve(fileId: string): IsoPoint | null;
  /**
   * AmbientLayer.release: take possession of the nearest idle crowd walker of
   * `figure` near `nearIso`. Returns its clean node-anchored handoff, or null
   * when no idle walker of that type exists (→ controller falls back to
   * spawn-fresh). MUST increment the layer's claimedCount on success.
   */
  release(figure: CitizenType, nearIso: IsoPoint): { pos: IsoPoint; nodeId: string } | null;
  /**
   * AmbientLayer.adopt: return a previously-claimed omino of `figure` to the
   * roaming crowd at/near `pos`. MUST decrement the layer's claimedCount.
   * Called EXACTLY once per successful release, only on the vanish of a
   * claim-from-crowd agent. A NULL `pos` (neither building nor agentPos resolved)
   * still decrements claimedCount but skips re-insertion — the controller must NOT
   * fabricate a fake position (#5).
   */
  adopt(figure: CitizenType, pos: IsoPoint | null): void;
  /**
   * The current ISO position of a placed agent's omino (its last-known anchor),
   * or null if AgentLayer has no such omino. Used as the adopt position so the
   * walker rejoins the crowd where the agent ended.
   */
  agentPos(agentId: string): IsoPoint | null;
}

/** How an agent's omino was brought onto the map — drives its release behaviour. */
export type ClaimOrigin = "claimed-from-crowd" | "spawned-fresh";

/** A decision the renderer applies against AgentLayer. The controller never
 *  touches PIXI; it only emits these + drives env.release/env.adopt. */
export type PossessionDecision =
  // Create a real-agent omino starting AT `startPos` (a crowd walker just
  // released there) and walk it toward `targetFileId`. No appear-fade: it
  // steps straight out of the crowd.
  | {
      kind: "createClaimed";
      agentId: string;
      agent: Agent;
      startPos: IsoPoint;
      startNodeId: string;
      targetFileId: string;
      targetIso: IsoPoint;
    }
  // Create a real-agent omino fresh AT its target building (existing appear-fade).
  | {
      kind: "createFresh";
      agentId: string;
      agent: Agent;
      targetFileId: string;
      targetIso: IsoPoint;
    }
  // An already-placed agent's data changed (status/task/type) but it stayed put.
  | { kind: "refresh"; agentId: string; agent: Agent }
  // An already-placed agent moved to a different building → walk it there.
  | {
      kind: "walk";
      agentId: string;
      agent: Agent;
      targetFileId: string;
      targetIso: IsoPoint;
    }
  // The agent vanished → destroy its omino. (Any adopt already happened in the
  // controller before this decision was emitted.)
  | { kind: "release"; agentId: string }
  // Spawn a small subagent omino near its parent at `pos` (fade-in).
  | {
      kind: "spawnSub";
      subId: string;
      parentAgentId: string;
      role: string;
      figure: CitizenType;
      pos: IsoPoint;
    }
  // Snap-reposition a subagent omino (its parent changed building) — no fade.
  | { kind: "moveSub"; subId: string; pos: IsoPoint }
  // Remove a subagent omino (fade-out + destroy).
  | { kind: "removeSub"; subId: string };

export interface PossessionResult {
  decisions: PossessionDecision[];
  /** How many subagent omini were dropped due to the per-parent cap this diff
   *  (for logging — never silently swallow an absurd count). */
  droppedSubagents: number;
}

/** Internal per-placed-agent record the controller tracks across diffs. */
interface AgentRecord {
  origin: ClaimOrigin;
  figure: CitizenType;
  fileId: string;
}

/** Internal per-subagent record: the parent it hangs off + its figure. */
interface SubRecord {
  parentAgentId: string;
  figure: CitizenType;
}

/**
 * Is an agent ON-MAP / ACTIVE — i.e. should it have an omino? Mirrors the
 * AgentLayer placement rule EXACTLY (so the two never disagree about who is on
 * the map): it must have a `currentFileId` that RESOLVES to a real building, and
 * it must not be the divine `augur`. The agent's status string is otherwise not
 * gated here — AgentLayer draws any placed agent and animates it per its real
 * status; "idle" is still a valid on-map pose (standing at its building).
 */
function isOnMap(agent: Agent, resolve: (fileId: string) => IsoPoint | null): IsoPoint | null {
  const fileId = agent.currentFileId;
  if (!fileId || agent.type === "augur") return null;
  return resolve(fileId);
}

/**
 * Deterministic ring offset for subagent slot #`index`. Pure function of the
 * INDEX ONLY: each slot owns a FIXED angle around the circle (a constant angular
 * step of `2π / MAX_SUBAGENT_OMINI_PER_PARENT` per slot), so slot 0 is ALWAYS at
 * the same spot regardless of how many siblings exist. This is the crux of the
 * stability contract: when a parent's subagent count changes (3→5 or 5→3) the
 * SURVIVOR slots (0..min-1) keep their exact offset — they never shuffle — and
 * newcomers simply occupy the next fixed positions. NO Math.random / Date.now and
 * NO dependence on `total`, so the offset is invariant frame-to-frame AND across
 * count changes. The radius is constant (slot-independent) for the same reason.
 */
export function subagentRingOffset(index: number): IsoPoint {
  // Fixed angular step per slot; a fixed phase so slot 0 isn't dead-on the right.
  const angle = (index / MAX_SUBAGENT_OMINI_PER_PARENT) * Math.PI * 2 + Math.PI / 6;
  return {
    x: Math.cos(angle) * SUBAGENT_RING_RADIUS,
    // ISO foreshortening: squash the vertical so the ring reads as a ground
    // circle around the building, not an upright wheel.
    y: Math.sin(angle) * SUBAGENT_RING_RADIUS * 0.5,
  };
}

/**
 * Stable, deterministic subagent omino id. Encodes the parent, the role, and the
 * per-role index so identity is stable across diffs (no flicker) and unique.
 * Format: `${parentAgentId}::sub::${role}::${i}`.
 */
export function subagentId(parentAgentId: string, role: string, index: number): string {
  return `${parentAgentId}::sub::${role}::${index}`;
}

/**
 * Derive the FLAT, capped, deterministically-ordered list of subagent omino
 * specs for one parent agent. Roles are taken in declared order; within a role
 * the omini are indexed 0..count-1. The per-parent cap bounds the total; overflow
 * (anything past {@link MAX_SUBAGENT_OMINI_PER_PARENT}) is dropped. Returns the
 * kept specs + how many were dropped.
 *
 * Each spec carries a STABLE id, its figure, and its ring offset (so the parent's
 * building position + offset gives the final omino position). The ring offset is a
 * function of the GLOBAL slot index ONLY (a fixed angle per slot), so a survivor's
 * slot keeps the SAME spot when the count changes — survivors never shuffle and
 * newcomers take the next fixed positions.
 */
export function deriveSubagents(parent: Agent): {
  specs: { subId: string; role: string; figure: CitizenType; offset: IsoPoint }[];
  dropped: number;
} {
  const briefs: AgentSubagentBrief[] = parent.subagents ?? [];
  // Polis-P6 FIX (#7) — assign the ring slot from a STABLE role order, NOT the
  // backend's declared brief order. The backend does not guarantee a stable brief
  // ordering across diffs, and even if it did, INSERTING a new role would otherwise
  // shift every later sub's flat slot (and thus its ring offset) → a spurious
  // moveSub storm for subs that didn't change. Sorting the per-role counts by role
  // string ascending makes each existing role's band depend only on the roles that
  // sort BEFORE it, not on declaration order — so re-ordering the SAME role set is a
  // no-op, and appending a role that sorts last leaves every existing sub's offset
  // untouched. (Counts for a duplicated role are merged so its indices stay
  // contiguous.) A single role still yields slot == perRoleIndex, preserving the
  // index-only ring contract the survivor-stability tests pin.
  const countByRole = new Map<string, number>();
  for (const brief of briefs) {
    const count = Math.max(0, Math.floor(brief.count));
    if (count <= 0) continue;
    countByRole.set(brief.role, (countByRole.get(brief.role) ?? 0) + count);
  }
  const roles = [...countByRole.keys()].sort();
  // Flatten to (role, perRoleIndex) in canonical (role-ascending) order, applying
  // the per-parent cap as a global ceiling so the dropped count is exact.
  const flat: { role: string; index: number }[] = [];
  let dropped = 0;
  for (const role of roles) {
    const count = countByRole.get(role) ?? 0;
    for (let i = 0; i < count; i++) {
      if (flat.length < MAX_SUBAGENT_OMINI_PER_PARENT) {
        flat.push({ role, index: i });
      } else {
        dropped += 1;
      }
    }
  }
  const specs = flat.map((f, slot) => ({
    subId: subagentId(parent.agentId, f.role, f.index),
    role: f.role,
    figure: figureForRole(f.role),
    // INDEX-ONLY offset: slot 0 is always at the same angle, so survivors keep
    // their spot when the total changes (no re-layout, no overlap on count churn).
    offset: subagentRingOffset(slot),
  }));
  return { specs, dropped };
}

export class PossessionController {
  // Placed REAL agents, keyed by agentId. Tracks how each was brought on-map so
  // its vanish does the right thing (adopt iff claim-from-crowd).
  private agents = new Map<string, AgentRecord>();
  // Placed SUBAGENT omini, keyed by their stable subId.
  private subs = new Map<string, SubRecord>();

  /** How many real-agent omini the controller currently believes are placed. */
  get placedCount(): number {
    return this.agents.size;
  }

  /** How many subagent omini the controller currently believes are placed. */
  get subagentCount(): number {
    return this.subs.size;
  }

  /** Origin of a placed agent (for tests / assertions); undefined if not placed. */
  originOf(agentId: string): ClaimOrigin | undefined {
    return this.agents.get(agentId)?.origin;
  }

  /**
   * Reconcile the placed set against the live `city.agents`. Emits the decisions
   * the renderer applies and drives env.release/env.adopt for the claimedCount
   * contract. Deterministic + idempotent: an identical input reconciled twice
   * after the first apply yields only `refresh` decisions (no churn, no re-claim).
   */
  reconcile(liveAgents: readonly Agent[], env: PossessionEnv): PossessionResult {
    const decisions: PossessionDecision[] = [];
    let droppedSubagents = 0;

    // Polis-P6 — running count of subagent omini ADMITTED so far this diff, against
    // the GLOBAL cap. Parents are processed in `liveAgents` order and slots in
    // declared order, so the drop is deterministic + stable: once this hits
    // MAX_SUBAGENT_OMINI_GLOBAL every further omino is dropped (the highest-index
    // parents/slots), and an identical input always drops the identical omini.
    let globalSubCount = 0;

    // The set of on-map agentIds this diff (for the removal sweep). Subagent ids
    // are tracked separately.
    const seenAgents = new Set<string>();
    const seenSubs = new Set<string>();

    for (const agent of liveAgents) {
      const targetIso = isOnMap(agent, env.resolve);
      const fileId = agent.currentFileId;
      // Off-map (no building / augur): it gets no omino. If it WAS placed, it is
      // treated as a removal below (not in seenAgents). Skip subagent derivation
      // too — a subagent rings the parent's BUILDING, which doesn't exist off-map.
      if (!targetIso || !fileId) continue;

      seenAgents.add(agent.agentId);

      const existing = this.agents.get(agent.agentId);
      // Did this already-placed parent change building this diff? Captured BEFORE
      // the walk branch mutates `existing.fileId` so the subagent loop can re-ring
      // its omini at the new building (a moveSub, no fade churn).
      const parentMoved = !!existing && existing.fileId !== fileId;
      if (!existing) {
        // NEW on-map agent → CLAIM from the crowd, else SPAWN-FRESH.
        const figure = figureForAgent(agent);
        const handoff = env.release(figure, targetIso);
        if (handoff) {
          this.agents.set(agent.agentId, {
            origin: "claimed-from-crowd",
            figure,
            fileId,
          });
          decisions.push({
            kind: "createClaimed",
            agentId: agent.agentId,
            agent,
            startPos: handoff.pos,
            startNodeId: handoff.nodeId,
            targetFileId: fileId,
            targetIso,
          });
        } else {
          this.agents.set(agent.agentId, {
            origin: "spawned-fresh",
            figure,
            fileId,
          });
          decisions.push({
            kind: "createFresh",
            agentId: agent.agentId,
            agent,
            targetFileId: fileId,
            targetIso,
          });
        }
      } else if (existing.fileId !== fileId) {
        // MOVED to a different building → WALK, never re-claim. Keep its origin
        // (a claimed agent that walks is still claimed; its eventual vanish must
        // still adopt). Refresh the tracked figure in case the agent's type/parent
        // flipped (e.g. a parentAgentId appeared → watercarrier).
        existing.fileId = fileId;
        existing.figure = figureForAgent(agent);
        decisions.push({
          kind: "walk",
          agentId: agent.agentId,
          agent,
          targetFileId: fileId,
          targetIso,
        });
      } else {
        // SAME building → just refresh data/pose. Keep its origin + update figure.
        existing.figure = figureForAgent(agent);
        decisions.push({ kind: "refresh", agentId: agent.agentId, agent });
      }

      // ---- Subagent derivation (option (b): spawn-direct, no crowd churn) ----
      const { specs, dropped } = deriveSubagents(agent);
      droppedSubagents += dropped;
      for (const spec of specs) {
        // Polis-P6 — GLOBAL cap: once the diff-wide budget is exhausted, DROP this
        // omino (don't seenSubs it, so an existing-but-now-over-budget sub is torn
        // down in the removal sweep below). Deterministic + stable: parents/slots
        // are visited in a fixed order, so the same omini always overflow.
        if (globalSubCount >= MAX_SUBAGENT_OMINI_GLOBAL) {
          droppedSubagents += 1;
          continue;
        }
        globalSubCount += 1;
        seenSubs.add(spec.subId);
        const pos = { x: targetIso.x + spec.offset.x, y: targetIso.y + spec.offset.y };
        if (!this.subs.has(spec.subId)) {
          // NEW subagent slot → spawn it directly near the parent (fade-in).
          this.subs.set(spec.subId, {
            parentAgentId: agent.agentId,
            figure: spec.figure,
          });
          decisions.push({
            kind: "spawnSub",
            subId: spec.subId,
            parentAgentId: agent.agentId,
            role: spec.role,
            figure: spec.figure,
            pos,
          });
        } else if (parentMoved) {
          // EXISTING subagent whose parent changed building → re-ring it at the new
          // building (a snap reposition, NOT a fade churn). A pure non-move diff
          // emits nothing for an existing sub (stable identity, no per-frame churn);
          // the deterministic ring offset keeps the same slot in the same spot.
          decisions.push({ kind: "moveSub", subId: spec.subId, pos });
        }
      }
    }

    // ---- Removal sweep: agents that vanished from the on-map live set ----
    for (const [agentId, rec] of this.agents) {
      if (seenAgents.has(agentId)) continue;
      // RELEASE the omino. If it was claim-from-crowd, return the figure to the
      // roaming crowd at its last position (the "torna a vagare" UX) — EXACTLY one
      // adopt per claim. A spawned-fresh agent took no crowd figure → NO adopt.
      if (rec.origin === "claimed-from-crowd") {
        // EXACTLY one adopt per claim. Prefer the BUILDING ANCHOR — it is always
        // node-anchored, so AmbientLayer.adopt's nearestNodeId snap is reliable.
        // A WALKING agent's agentPos returns an interpolated MID-SEGMENT point
        // (not a road node), which a long segment can snap wrong / fail to seat,
        // leaving the crowd permanently one short. Fall back to agentPos only when
        // the building no longer resolves (off-map), then {0,0} as a last resort.
        // Either way the call MUST happen so claimedCount is balanced —
        // AmbientLayer.adopt floors the count and no-ops the re-insert if it cannot
        // snap a node (the gone-building / full-teardown case, which clear() heals).
        // #5 — if NEITHER the building anchor NOR agentPos resolves, pass null (do
        // NOT fabricate {0,0}: that would snap the returned walker to a random
        // near-origin node — spatially dishonest). adopt still decrements the
        // claimedCount on null, so the accounting stays balanced either way.
        const lastPos = env.resolve(rec.fileId) ?? env.agentPos(agentId);
        env.adopt(rec.figure, lastPos);
      }
      this.agents.delete(agentId);
      decisions.push({ kind: "release", agentId });
    }

    // ---- Removal sweep: subagents whose slot no longer exists ----
    // #11 — iterate the Map directly and delete in place (deleting the current key
    // during a Map for-of is well-defined in JS), mirroring the agent removal sweep
    // above. Avoids the per-reconcile `[...this.subs.keys()]` array allocation.
    for (const subId of this.subs.keys()) {
      if (seenSubs.has(subId)) continue;
      this.subs.delete(subId);
      decisions.push({ kind: "removeSub", subId });
    }

    return { decisions, droppedSubagents };
  }

  /**
   * Full reset (a city reload / teardown). Drops ALL tracked state WITHOUT any
   * adopt — the matching reset is AmbientLayer.clear() (which zeroes claimedCount).
   * Pairs with the renderer clearing both layers on clearScene().
   */
  clear(): void {
    this.agents.clear();
    this.subs.clear();
  }
}

// Re-export the hash so a consumer that wants a stable per-subagent seed (e.g. a
// tunic variation) can derive it from the subId without importing rng directly.
export { hashString };
