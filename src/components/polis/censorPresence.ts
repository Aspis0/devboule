// censorPresence.ts — Polis-P5 Censor firefighter presence (PURE decision layer).
//
// Censor is an ENGINE, not an agent. It must NEVER appear in `city.agents`, the
// fleet roster, or the PossessionController agent diff. Instead it drives ONE
// roaming "firefighter" omino directly, keyed by a stable id (`censor:<projectId>`),
// off the REAL `censor://findings-updated` event — no fabricated agent session.
//
// This module is PURE and headless-testable, exactly like possession.ts: it holds
// NO PIXI, NO real clock, and NO Math.random / Date.now. The debounce uses an
// INJECTED clock (caller passes `nowMs`); the impure shell (PolisView) owns the
// actual setTimeout / Tauri subscription. The decision core is deterministic: the
// same event sequence + the same clock yields the same decisions.
//
// ┌─────────────────────────────────────────────────────────────────────────┐
// │ BEHAVIOUR                                                                 │
// │  - A `findings-updated` event naming resolvable file(s) while gemma is    │
// │    NOT offline → CLAIM an idle firefighter from the crowd (env.release;   │
// │    fallback spawn-fresh) and WALK it to the reviewed building (the first  │
// │    resolvable file, deterministically). While reviewing → extinguishing.  │
// │  - The feed SETTLES (an empty-`files` event, or the debounce quiesces with │
// │    no naming event) OR gemma goes OFFLINE → stop extinguishing + RELEASE   │
// │    the firefighter back to roaming (env.adopt — one adopt per claim, never │
// │    for a fallback spawn-fresh).                                            │
// │  - ONE firefighter per project: consecutive events naming a DIFFERENT      │
// │    building WALK the existing firefighter (no re-claim, no extra omino).   │
// │  - Unresolvable relPaths are DROPPED (never fabricate a building). If NO    │
// │    file resolves, no omino (don't claim).                                  │
// └─────────────────────────────────────────────────────────────────────────┘
//
// claimedCount CONTRACT (mirrors possession.ts):
//   - EXACTLY ONE env.adopt per successful env.release (claim-from-crowd).
//   - NEVER an env.adopt for a fallback spawn-fresh firefighter.
//   - On release-to-roaming the controller drives env.adopt itself (iff claimed),
//     then emits a `destroy` decision the renderer applies against AgentLayer.

import type { IsoPoint } from "./iso";

/** The firefighter kit figure the Censor presence renders. A constant (not a
 *  per-agent map) — Censor is the ONLY producer of a firefighter omino, and no
 *  real agent `type` maps to it (see AgentLayer.figureForType). */
export const CENSOR_FIGURE = "firefighter" as const;

/** Default debounce window (ms) the engine's burst of `findings-updated` events
 *  is coalesced over before the firefighter reacts, so it doesn't flicker across
 *  the several emits of a single review pass. The IMPURE shell passes `nowMs`; the
 *  PURE core compares against this deadline. Kept short so the tell stays snappy. */
export const CENSOR_DEBOUNCE_MS = 450;

/** The `censor://findings-updated` payload shape (camelCase from Rust). `files`
 *  are project-relative, forward-slash-normalized paths; an EMPTY `files` is a
 *  SETTLE signal (the pass produced no shard change / the review quiesced). */
export interface CensorFindingsPayload {
  projectId: string;
  files: string[];
}

/** Gemma availability tri-state surfaced by `censor_status` — the firefighter is
 *  suppressed (and released if present) when the engine is OFFLINE. "unknown"
 *  (not yet probed) is treated as NOT offline so a first event still reacts. */
export type GemmaStatus = "available" | "offline" | "unknown";

/** A resolved building: its stable fileId + iso anchor. Mirrors the agents'
 *  resolution (`buildingNodes.get(fileId)?.iso`) so the firefighter walks to the
 *  exact same anchor a real agent would. */
export interface ResolvedBuilding {
  fileId: string;
  iso: IsoPoint;
}

/**
 * The environment the PURE controller talks to — abstracts the AmbientLayer claim
 * primitives + the relPath→building resolution so the controller is unit-testable
 * with a plain mock (no PIXI). MIRRORS PossessionEnv where it overlaps so the
 * claimedCount accounting is identical.
 */
export interface CensorEnv {
  /**
   * Resolve a project-relative path to its building (fileId + iso), or null when
   * the path names no rendered building. The SAME resolution agents use — the
   * renderer matches the normalized relPath against `building.filePath`. Drop
   * unresolvable paths; never fabricate a building.
   */
  resolveRelPath(relPath: string): ResolvedBuilding | null;
  /**
   * AmbientLayer.release: take possession of the nearest idle firefighter near
   * `nearIso`. Returns its handoff (start pos + road node), or null when no idle
   * firefighter exists (→ controller falls back to spawn-fresh). MUST increment
   * the layer's claimedCount on success.
   */
  release(nearIso: IsoPoint): { pos: IsoPoint; nodeId: string } | null;
  /**
   * AmbientLayer.adopt: return the previously-claimed firefighter to the roaming
   * crowd at/near `pos`. MUST decrement claimedCount. Called EXACTLY once per
   * successful release, only on release-to-roaming of a claim-from-crowd omino.
   */
  adopt(pos: IsoPoint): void;
  /**
   * The current ISO anchor of the Censor firefighter omino (AgentLayer's last
   * known position for it), or null if none placed. Used as the adopt position so
   * the walker rejoins the crowd where the firefighter ended.
   */
  firefighterPos(): IsoPoint | null;
}

/** How the firefighter omino was brought on-map — drives its release behaviour. */
export type CensorOrigin = "claimed-from-crowd" | "spawned-fresh";

/** A decision the renderer applies against AgentLayer. The controller never
 *  touches PIXI; it only emits these + drives env.release/env.adopt. */
export type CensorDecision =
  // Create a firefighter omino starting AT `startPos` (a crowd walker just
  // released there) and walk it toward `targetFileId`. No appear-fade.
  | {
      kind: "createClaimed";
      startPos: IsoPoint;
      startNodeId: string;
      targetFileId: string;
      targetIso: IsoPoint;
    }
  // Create a firefighter omino fresh AT its target building (appear-fade).
  | {
      kind: "createFresh";
      targetFileId: string;
      targetIso: IsoPoint;
    }
  // The (already placed) firefighter moved to a different reviewed building → walk.
  | { kind: "walk"; targetFileId: string; targetIso: IsoPoint }
  // Toggle the water-arc tell (the P2 `extinguishing` gate on the firefighter).
  | { kind: "extinguishing"; on: boolean }
  // The firefighter is released back to roaming → destroy its omino. (Any adopt
  // already happened in the controller before this decision was emitted.)
  | { kind: "destroy" };

/** Internal record for the single tracked firefighter. */
interface FirefighterRecord {
  origin: CensorOrigin;
  /** The building it is currently AT (or walking toward). */
  fileId: string;
  /** Whether the water-arc tell is currently on (so we don't re-emit no-ops). */
  extinguishing: boolean;
}

/**
 * Polis-P5 — the PURE Censor firefighter controller. Stateful (it tracks the one
 * placed firefighter) but free of PIXI / real clock / randomness — exactly the
 * posture of PossessionController. The IMPURE shell feeds it events + a clock and
 * applies its decisions against AgentLayer.
 */
export class CensorPresence {
  /** The placed firefighter, or null when none is on-map. ONE per project. */
  private fire: FirefighterRecord | null = null;
  /** Latest gemma tri-state. Offline suppresses + releases the firefighter. */
  private gemma: GemmaStatus = "unknown";
  /** The pending (debounced) NAMING event: the resolvable target chosen from its
   *  files, plus the deadline at which it should be flushed. Null when nothing is
   *  pending. A SETTLE (empty-files) event is applied immediately, not debounced. */
  private pending: { target: ResolvedBuilding; deadlineMs: number } | null = null;

  /** Is a firefighter currently placed? (For tests / assertions.) */
  get placed(): boolean {
    return this.fire !== null;
  }

  /** Origin of the placed firefighter (for tests); undefined if none placed. */
  get origin(): CensorOrigin | undefined {
    return this.fire?.origin;
  }

  /** Whether the water-arc tell is currently on (for tests). */
  get extinguishing(): boolean {
    return this.fire?.extinguishing ?? false;
  }

  /** The building the firefighter is currently at (for tests); undefined if none. */
  get fileId(): string | undefined {
    return this.fire?.fileId;
  }

  /** Whether a debounced naming event is waiting to flush (for tests / the shell
   *  to know whether to (re)arm a settle timer). */
  get hasPending(): boolean {
    return this.pending !== null;
  }

  /** The deadline (ms, on the injected clock) at which the pending event flushes,
   *  or null. The shell uses this to schedule its setTimeout. */
  get pendingDeadlineMs(): number | null {
    return this.pending?.deadlineMs ?? null;
  }

  /**
   * Update the cached gemma status. When it flips to OFFLINE, any placed
   * firefighter is released to roaming (and any pending event dropped) — the
   * engine isn't running, so the tell must not show. Returns the decisions to
   * apply (a release sequence, or empty).
   */
  setGemmaStatus(status: GemmaStatus, env: CensorEnv): CensorDecision[] {
    this.gemma = status;
    if (status === "offline") {
      // Offline: drop any pending naming event and release the firefighter.
      this.pending = null;
      return this.releaseFirefighter(env);
    }
    return [];
  }

  /**
   * Ingest a `findings-updated` event at `nowMs` (the injected clock). Returns the
   * decisions to apply NOW. Behaviour:
   *   - gemma OFFLINE → no-op (the firefighter, if any, was released when gemma
   *     flipped; an event while offline never spawns one).
   *   - EMPTY files (a SETTLE signal) → release the firefighter immediately
   *     (debounce is for naming bursts, not for the settle — settling should be
   *     prompt so the tell clears).
   *   - NAMING files with at least one RESOLVABLE path → arm/refresh the debounce
   *     toward the FIRST resolvable building. No decision is emitted until the
   *     debounce flushes in {@link tick}, so a burst coalesces into one reaction.
   *   - NAMING files but NONE resolvable → DROP (never fabricate a building; if a
   *     firefighter is already placed it is left untouched — an unresolvable pass
   *     is not a settle).
   */
  onFindings(
    payload: CensorFindingsPayload,
    nowMs: number,
    env: CensorEnv,
  ): CensorDecision[] {
    if (this.gemma === "offline") return [];

    // EMPTY files = settle. Clear any pending naming event and release promptly.
    if (payload.files.length === 0) {
      this.pending = null;
      return this.releaseFirefighter(env);
    }

    // Pick the FIRST resolvable file deterministically (input order is stable —
    // the backend sorts the files; we honour that order and take the first hit).
    let target: ResolvedBuilding | null = null;
    for (const rel of payload.files) {
      const r = env.resolveRelPath(rel);
      if (r) {
        target = r;
        break;
      }
    }
    // No file resolves → drop (don't fabricate, don't settle, don't claim).
    if (!target) return [];

    // Arm/refresh the debounce toward this target; the reaction flushes in tick().
    this.pending = { target, deadlineMs: nowMs + CENSOR_DEBOUNCE_MS };
    return [];
  }

  /**
   * Advance the debounce clock to `nowMs`. If a pending naming event's deadline
   * has elapsed (a burst has quiesced), FLUSH it: claim/walk the firefighter to
   * the pending target and turn the extinguishing tell on. Returns the decisions
   * to apply (empty until the deadline passes). Idempotent once flushed (the
   * pending is cleared). The IMPURE shell calls this from its settle timer.
   */
  tick(nowMs: number, env: CensorEnv): CensorDecision[] {
    if (this.gemma === "offline") {
      // Defensive: should already be released, but never act on a stale pending.
      this.pending = null;
      return [];
    }
    const p = this.pending;
    if (!p || nowMs < p.deadlineMs) return [];
    this.pending = null;
    return this.reviewAt(p.target, env);
  }

  // -------------------------------------------------------------------------
  // Internal decision builders
  // -------------------------------------------------------------------------

  /**
   * Bring the firefighter to `target` and turn extinguishing on. Claims from the
   * crowd (else spawns fresh) when none is placed; WALKS the existing firefighter
   * (no re-claim) when it is already placed at a different building; just ensures
   * the tell is on when it is already at the target.
   */
  private reviewAt(target: ResolvedBuilding, env: CensorEnv): CensorDecision[] {
    const decisions: CensorDecision[] = [];

    if (!this.fire) {
      // NEW → CLAIM from the crowd, else SPAWN-FRESH.
      const handoff = env.release(target.iso);
      if (handoff) {
        this.fire = {
          origin: "claimed-from-crowd",
          fileId: target.fileId,
          extinguishing: false,
        };
        decisions.push({
          kind: "createClaimed",
          startPos: handoff.pos,
          startNodeId: handoff.nodeId,
          targetFileId: target.fileId,
          targetIso: target.iso,
        });
      } else {
        this.fire = {
          origin: "spawned-fresh",
          fileId: target.fileId,
          extinguishing: false,
        };
        decisions.push({
          kind: "createFresh",
          targetFileId: target.fileId,
          targetIso: target.iso,
        });
      }
    } else if (this.fire.fileId !== target.fileId) {
      // MOVED to a different reviewed building → WALK, never re-claim. Keep its
      // origin (a claimed firefighter that walks is still claimed; its eventual
      // release must still adopt).
      this.fire.fileId = target.fileId;
      decisions.push({
        kind: "walk",
        targetFileId: target.fileId,
        targetIso: target.iso,
      });
    }
    // SAME building → nothing to move; just ensure the tell is on below.

    // Turn the water-arc tell ON (idempotent — only emit on an actual change).
    if (this.fire && !this.fire.extinguishing) {
      this.fire.extinguishing = true;
      decisions.push({ kind: "extinguishing", on: true });
    }
    return decisions;
  }

  /**
   * Release the firefighter back to roaming: stop extinguishing, drive env.adopt
   * IFF it was claim-from-crowd (one adopt per claim, none for spawn-fresh), then
   * emit a destroy decision. No-op (empty) when none is placed.
   */
  private releaseFirefighter(env: CensorEnv): CensorDecision[] {
    const fire = this.fire;
    if (!fire) return [];
    const decisions: CensorDecision[] = [];
    // Stop the tell first so the released walker doesn't carry a water arc into
    // the crowd (defensive — the omino is destroyed below, but keep state honest).
    if (fire.extinguishing) {
      decisions.push({ kind: "extinguishing", on: false });
    }
    // EXACTLY one adopt per claim. A spawned-fresh firefighter took no crowd
    // figure → NO adopt (adopting would inflate the crowd). Adopt at the omino's
    // last position so it rejoins the crowd where it ended; {0,0} as a last resort
    // (AmbientLayer.adopt floors claimedCount + no-ops the re-insert if it can't
    // snap a node, so the call MUST happen to keep the count balanced).
    if (fire.origin === "claimed-from-crowd") {
      const pos = env.firefighterPos() ?? { x: 0, y: 0 };
      env.adopt(pos);
    }
    this.fire = null;
    decisions.push({ kind: "destroy" });
    return decisions;
  }

  /**
   * Release the firefighter back to roaming WITHOUT changing the gemma status —
   * used for a PROJECT SWITCH (a findings event for a different project): the old
   * project's firefighter must leave, but the cached gemma availability is left
   * intact so the new project's first event reacts against the real (last-pushed)
   * status. Also drops any pending naming event. One adopt per claim (none for a
   * spawn-fresh), exactly like the settle/offline paths.
   */
  releaseForSwitch(env: CensorEnv): CensorDecision[] {
    this.pending = null;
    return this.releaseFirefighter(env);
  }

  /**
   * Full reset (a city reload / unmount). Drops ALL tracked state WITHOUT any
   * adopt — the matching reset is AmbientLayer.clear() (which zeroes claimedCount),
   * paired with the renderer clearing the layers on clearScene(). Mirrors
   * PossessionController.clear().
   */
  clear(): void {
    this.fire = null;
    this.pending = null;
    // gemma is intentionally LEFT as-is: a city reload doesn't re-probe gemma, so
    // the last-known availability still gates a post-reload event correctly.
  }
}
