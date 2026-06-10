import { describe, it, expect } from "vitest";
import {
  PossessionController,
  deriveSubagents,
  subagentId,
  subagentRingOffset,
  MAX_SUBAGENT_OMINI_PER_PARENT,
  MAX_SUBAGENT_OMINI_GLOBAL,
  type PossessionEnv,
  type PossessionDecision,
} from "./possession";
import type { Agent, AgentSubagentBrief } from "../../types/city";
import type { IsoPoint } from "./iso";
import type { CitizenType } from "./kitcd/people";

// Polis-P4 — the PURE PossessionController, tested HEADLESSLY against a mock
// PossessionEnv (no PIXI, no clock, no Math.random/Date.now). The env records
// every release/adopt so the claimedCount contract (one adopt per claim, none for
// a fallback spawn or a subagent) can be asserted exactly.

// ---------------------------------------------------------------------------
// Test world + mock env
// ---------------------------------------------------------------------------

const POS: Record<string, IsoPoint> = {
  f1: { x: 100, y: 50 },
  f2: { x: 200, y: 80 },
  f3: { x: 300, y: 120 },
};

interface ReleaseCall {
  figure: CitizenType;
  nearIso: IsoPoint;
}
interface AdoptCall {
  figure: CitizenType;
  pos: IsoPoint | null;
}

/**
 * A controllable mock env. `idleFigures` is the multiset of crowd figure types
 * that are currently CLAIMABLE; `release` consumes one matching entry (returning a
 * deterministic handoff) or null. `agentPositions` feeds `agentPos`.
 */
function makeEnv(opts?: {
  idleFigures?: CitizenType[];
  agentPositions?: Record<string, IsoPoint>;
}) {
  const idle = [...(opts?.idleFigures ?? [])];
  const agentPositions: Record<string, IsoPoint> = { ...(opts?.agentPositions ?? {}) };
  const releaseCalls: ReleaseCall[] = [];
  const adoptCalls: AdoptCall[] = [];
  let handoffSeq = 0;

  const env: PossessionEnv = {
    resolve(fileId) {
      return POS[fileId] ?? null;
    },
    release(figure, nearIso) {
      releaseCalls.push({ figure, nearIso });
      const idx = idle.indexOf(figure);
      if (idx < 0) return null;
      idle.splice(idx, 1);
      handoffSeq += 1;
      // A deterministic handoff position offset from the target so tests can tell
      // a claimed start pos from the target building pos.
      return {
        pos: { x: nearIso.x - 10, y: nearIso.y - 10 },
        nodeId: `node${handoffSeq}`,
      };
    },
    adopt(figure, pos) {
      adoptCalls.push({ figure, pos });
    },
    agentPos(agentId) {
      return agentPositions[agentId] ?? null;
    },
  };

  return { env, releaseCalls, adoptCalls, agentPositions };
}

function mkAgent(over: Partial<Agent>): Agent {
  return {
    agentId: "a1",
    type: "coder",
    status: "working",
    currentFileId: "f1",
    currentTask: null,
    color: "#888888",
    ...over,
  };
}

function only<K extends PossessionDecision["kind"]>(
  decisions: PossessionDecision[],
  kind: K,
): Extract<PossessionDecision, { kind: K }>[] {
  return decisions.filter((d) => d.kind === kind) as Extract<
    PossessionDecision,
    { kind: K }
  >[];
}

// ---------------------------------------------------------------------------
// Claim vs spawn-fresh
// ---------------------------------------------------------------------------

describe("PossessionController — new active agent lifecycle", () => {
  it("CLAIMS from the crowd when an idle figure of its type is available", () => {
    const { env, releaseCalls, adoptCalls } = makeEnv({ idleFigures: ["builder"] });
    const ctrl = new PossessionController();
    const agent = mkAgent({ type: "coder", currentFileId: "f1" });

    const { decisions } = ctrl.reconcile([agent], env);

    const claims = only(decisions, "createClaimed");
    expect(claims).toHaveLength(1);
    expect(claims[0].agentId).toBe("a1");
    // figureForAgent(coder) === builder, so release asked for a builder.
    expect(releaseCalls).toEqual([{ figure: "builder", nearIso: POS.f1 }]);
    // Starts at the handoff pos (offset from target), NOT at the building.
    expect(claims[0].startPos).toEqual({ x: POS.f1.x - 10, y: POS.f1.y - 10 });
    expect(claims[0].targetIso).toEqual(POS.f1);
    expect(ctrl.originOf("a1")).toBe("claimed-from-crowd");
    // No adopt yet — the claim hasn't ended.
    expect(adoptCalls).toHaveLength(0);
  });

  it("SPAWNS FRESH at the target when no idle figure of its type exists", () => {
    const { env, releaseCalls, adoptCalls } = makeEnv({ idleFigures: [] });
    const ctrl = new PossessionController();
    const agent = mkAgent({ type: "coder", currentFileId: "f1" });

    const { decisions } = ctrl.reconcile([agent], env);

    expect(only(decisions, "createClaimed")).toHaveLength(0);
    const fresh = only(decisions, "createFresh");
    expect(fresh).toHaveLength(1);
    expect(fresh[0].targetIso).toEqual(POS.f1);
    // It still ATTEMPTED a release (which returned null) but adopted nothing.
    expect(releaseCalls).toHaveLength(1);
    expect(adoptCalls).toHaveLength(0);
    expect(ctrl.originOf("a1")).toBe("spawned-fresh");
  });

  it("does NOT place an off-map agent (augur / unresolved file) — no omino, no release", () => {
    const { env, releaseCalls } = makeEnv({ idleFigures: ["builder", "noble"] });
    const ctrl = new PossessionController();
    const augur = mkAgent({ agentId: "au", type: "augur", currentFileId: "f1" });
    const noFile = mkAgent({ agentId: "nf", currentFileId: null });
    const badFile = mkAgent({ agentId: "bf", currentFileId: "ghost" });

    const { decisions } = ctrl.reconcile([augur, noFile, badFile], env);

    expect(decisions).toHaveLength(0);
    expect(releaseCalls).toHaveLength(0);
    expect(ctrl.placedCount).toBe(0);
  });
});

// ---------------------------------------------------------------------------
// Building change → walk, no re-claim
// ---------------------------------------------------------------------------

describe("PossessionController — placed agent building change", () => {
  it("WALKS (no re-claim) when a placed agent's building changes", () => {
    const { env, releaseCalls } = makeEnv({ idleFigures: ["builder"] });
    const ctrl = new PossessionController();
    const a1 = mkAgent({ currentFileId: "f1" });

    ctrl.reconcile([a1], env); // initial claim
    expect(releaseCalls).toHaveLength(1);

    const moved = mkAgent({ currentFileId: "f2" });
    const { decisions } = ctrl.reconcile([moved], env);

    const walks = only(decisions, "walk");
    expect(walks).toHaveLength(1);
    expect(walks[0].targetFileId).toBe("f2");
    expect(walks[0].targetIso).toEqual(POS.f2);
    // NO second release — the walk is not a re-claim.
    expect(releaseCalls).toHaveLength(1);
    // Origin preserved across the move.
    expect(ctrl.originOf("a1")).toBe("claimed-from-crowd");
  });

  it("emits only a REFRESH when a placed agent stays at its building", () => {
    const { env } = makeEnv({ idleFigures: ["builder"] });
    const ctrl = new PossessionController();
    ctrl.reconcile([mkAgent({ currentFileId: "f1", status: "working" })], env);

    const { decisions } = ctrl.reconcile(
      [mkAgent({ currentFileId: "f1", status: "idle" })],
      env,
    );
    expect(only(decisions, "refresh")).toHaveLength(1);
    expect(only(decisions, "walk")).toHaveLength(0);
    expect(only(decisions, "createClaimed")).toHaveLength(0);
  });
});

// ---------------------------------------------------------------------------
// Vanish → release + adopt contract
// ---------------------------------------------------------------------------

describe("PossessionController — vanish releases the omino + the claimedCount contract", () => {
  it("a CLAIMED-FROM-CROWD agent vanishing → RELEASE + exactly ONE adopt at the NODE-ANCHORED building (F4)", () => {
    // F4: the adopt position prefers the building anchor (always node-anchored)
    // over agentPos, which for a WALKING agent is a mid-segment interpolation that
    // can snap wrong / fail to re-seat the walker. Here agentPos returns a
    // mid-walk point but the building (f1) resolves, so the building anchor wins.
    const midWalk = { x: 175, y: 65 }; // interpolated, not a road node
    const { env, adoptCalls } = makeEnv({
      idleFigures: ["builder"],
      agentPositions: { a1: midWalk },
    });
    const ctrl = new PossessionController();
    ctrl.reconcile([mkAgent({ currentFileId: "f1" })], env);

    const { decisions } = ctrl.reconcile([], env);

    expect(only(decisions, "release")).toEqual([{ kind: "release", agentId: "a1" }]);
    // Adopted at the node-anchored building, NOT at the mid-walk interpolation.
    expect(adoptCalls).toEqual([{ figure: "builder", pos: POS.f1 }]);
    expect(ctrl.placedCount).toBe(0);
  });

  it("a SPAWNED-FRESH agent vanishing → RELEASE, NO adopt (no crowd inflation)", () => {
    const { env, adoptCalls } = makeEnv({ idleFigures: [] }); // → spawn-fresh
    const ctrl = new PossessionController();
    ctrl.reconcile([mkAgent({ currentFileId: "f1" })], env);
    expect(ctrl.originOf("a1")).toBe("spawned-fresh");

    const { decisions } = ctrl.reconcile([], env);
    expect(only(decisions, "release")).toHaveLength(1);
    expect(adoptCalls).toHaveLength(0);
  });

  it("an agent that goes OFF-MAP (file→null) vanishes like a removal (adopt for claimed)", () => {
    const { env, adoptCalls } = makeEnv({
      idleFigures: ["builder"],
      agentPositions: { a1: POS.f1 },
    });
    const ctrl = new PossessionController();
    ctrl.reconcile([mkAgent({ currentFileId: "f1" })], env);

    // Same agent, now off-map.
    const { decisions } = ctrl.reconcile([mkAgent({ currentFileId: null })], env);
    expect(only(decisions, "release")).toHaveLength(1);
    expect(adoptCalls).toHaveLength(1);
  });

  it("uses the building anchor for adopt even when agentPos is unknown", () => {
    const { env, adoptCalls } = makeEnv({ idleFigures: ["builder"] }); // no agentPositions
    const ctrl = new PossessionController();
    ctrl.reconcile([mkAgent({ currentFileId: "f1" })], env);
    ctrl.reconcile([], env);
    expect(adoptCalls).toEqual([{ figure: "builder", pos: POS.f1 }]);
  });

  it("falls back to agentPos for adopt only when the building no longer resolves (F4)", () => {
    // If the building was torn down (resolve → null) the building anchor is
    // unavailable, so agentPos is the next-best position for the adopt.
    const fallbackPos = { x: 12, y: 34 };
    const { env, adoptCalls } = makeEnv({
      idleFigures: ["builder"],
      agentPositions: { a1: fallbackPos },
    });
    const ctrl = new PossessionController();
    ctrl.reconcile([mkAgent({ currentFileId: "f1" })], env);
    // Remove f1 from the resolvable buildings so the anchor is gone on vanish.
    const removed: PossessionEnv = {
      ...env,
      resolve: (fileId) => (fileId === "f1" ? null : env.resolve(fileId)),
    };
    ctrl.reconcile([], removed);
    expect(adoptCalls).toEqual([{ figure: "builder", pos: fallbackPos }]);
  });

  it("adopts with NULL pos (no {0,0} fabrication) when neither building nor agentPos resolves (#5)", () => {
    // Neither the building anchor (f1 removed) NOR agentPos is known on vanish. The
    // controller must still CALL adopt (so claimedCount stays balanced) but pass
    // null — never a fabricated {0,0} that would snap the walker to a random
    // near-origin node.
    const { env, adoptCalls } = makeEnv({ idleFigures: ["builder"] }); // no agentPositions
    const ctrl = new PossessionController();
    ctrl.reconcile([mkAgent({ currentFileId: "f1" })], env);
    const removed: PossessionEnv = {
      ...env,
      resolve: (fileId) => (fileId === "f1" ? null : env.resolve(fileId)),
    };
    ctrl.reconcile([], removed);
    // The adopt happened (balances the claim) but with an honest null position.
    expect(adoptCalls).toEqual([{ figure: "builder", pos: null }]);
  });
});

// ---------------------------------------------------------------------------
// No double-claim / idempotency / determinism
// ---------------------------------------------------------------------------

describe("PossessionController — no double-claim, stable identity, determinism", () => {
  it("re-reconciling an identical diff after apply yields only REFRESH (no churn)", () => {
    const { env, releaseCalls } = makeEnv({ idleFigures: ["builder"] });
    const ctrl = new PossessionController();
    const a1 = mkAgent({ currentFileId: "f1" });

    ctrl.reconcile([a1], env); // claim
    const { decisions } = ctrl.reconcile([a1], env); // identical → refresh only

    expect(only(decisions, "createClaimed")).toHaveLength(0);
    expect(only(decisions, "createFresh")).toHaveLength(0);
    expect(only(decisions, "refresh")).toHaveLength(1);
    expect(releaseCalls).toHaveLength(1); // no second claim
  });

  it("never double-claims even across many identical diffs", () => {
    const { env, releaseCalls, adoptCalls } = makeEnv({ idleFigures: ["builder"] });
    const ctrl = new PossessionController();
    const a1 = mkAgent({ currentFileId: "f1" });
    for (let i = 0; i < 5; i++) ctrl.reconcile([a1], env);
    expect(releaseCalls).toHaveLength(1);
    expect(adoptCalls).toHaveLength(0);
  });

  it("same input → same decisions on two independent controllers (determinism)", () => {
    const e1 = makeEnv({ idleFigures: ["builder", "citizen"] });
    const e2 = makeEnv({ idleFigures: ["builder", "citizen"] });
    const c1 = new PossessionController();
    const c2 = new PossessionController();
    const agents = [
      mkAgent({ agentId: "a1", type: "coder", currentFileId: "f1" }),
      mkAgent({ agentId: "a2", type: "verifier", currentFileId: "f2" }),
    ];
    const r1 = c1.reconcile(agents, e1.env);
    const r2 = c2.reconcile(agents, e2.env);
    expect(r1.decisions).toEqual(r2.decisions);
    expect(e1.releaseCalls).toEqual(e2.releaseCalls);
  });
});

// ---------------------------------------------------------------------------
// Subagent derivation
// ---------------------------------------------------------------------------

function subs(...entries: [string, number][]): AgentSubagentBrief[] {
  return entries.map(([role, count]) => ({ role, count }));
}

describe("deriveSubagents — pure derivation", () => {
  it("derives N stable-id omini for {coder:3} with deterministic offsets near the parent", () => {
    const parent = mkAgent({ agentId: "p", subagents: subs(["coder", 3]) });
    const { specs, dropped } = deriveSubagents(parent);
    expect(dropped).toBe(0);
    expect(specs.map((s) => s.subId)).toEqual([
      "p::sub::coder::0",
      "p::sub::coder::1",
      "p::sub::coder::2",
    ]);
    expect(specs.every((s) => s.figure === "builder")).toBe(true);
    // Offsets are the deterministic, INDEX-ONLY ring (no dependence on total).
    expect(specs.map((s) => s.offset)).toEqual([
      subagentRingOffset(0),
      subagentRingOffset(1),
      subagentRingOffset(2),
    ]);
  });

  it("enforces the per-parent cap and reports the dropped overflow", () => {
    const parent = mkAgent({ agentId: "p", subagents: subs(["coder", 10]) });
    const { specs, dropped } = deriveSubagents(parent);
    expect(specs).toHaveLength(MAX_SUBAGENT_OMINI_PER_PARENT);
    expect(dropped).toBe(10 - MAX_SUBAGENT_OMINI_PER_PARENT);
  });

  it("subagentId / subagentRingOffset are deterministic", () => {
    expect(subagentId("p", "coder", 2)).toBe("p::sub::coder::2");
    expect(subagentRingOffset(1)).toEqual(subagentRingOffset(1));
  });

  it("ring offset is INDEX-ONLY: survivors keep their slot when the total changes (F1)", () => {
    // The crux of the stability contract: slot 0 (and every survivor slot) must
    // resolve to the SAME offset regardless of how many siblings exist, so a 3→5
    // (or 5→3) count change never shuffles the survivors. With the old
    // total-dependent angle, offset(0) for total=3 differed from total=5.
    const parent3 = mkAgent({ agentId: "p", subagents: subs(["coder", 3]) });
    const parent5 = mkAgent({ agentId: "p", subagents: subs(["coder", 5]) });
    const o3 = deriveSubagents(parent3).specs;
    const o5 = deriveSubagents(parent5).specs;
    // The 3 survivors (slots 0,1,2) have byte-identical offsets across the change.
    for (let i = 0; i < 3; i++) {
      expect(o5[i].offset).toEqual(o3[i].offset);
      expect(o5[i].offset).toEqual(subagentRingOffset(i));
    }
  });

  it("ring slot is from a STABLE role order, not declared order — re-ordering the SAME role set is a no-op (#7)", () => {
    // The slot/offset for a given (role, perRoleIndex) must not depend on the
    // order the briefs arrive in (the backend doesn't guarantee a stable order).
    const a = mkAgent({ agentId: "p", subagents: subs(["coder", 2], ["verifier", 2]) });
    const b = mkAgent({ agentId: "p", subagents: subs(["verifier", 2], ["coder", 2]) });
    const sa = deriveSubagents(a).specs;
    const sb = deriveSubagents(b).specs;
    // Map subId → offset for an order-independent comparison.
    const offA = new Map(sa.map((s) => [s.subId, s.offset]));
    const offB = new Map(sb.map((s) => [s.subId, s.offset]));
    expect([...offA.keys()].sort()).toEqual([...offB.keys()].sort());
    for (const [id, off] of offA) expect(offB.get(id)).toEqual(off);
  });

  it("adding a new role does NOT change the existing subs' offsets (#7)", () => {
    // "coder" sorts before "verifier", so appending a verifier role leaves every
    // existing coder sub's ring slot (and offset) byte-identical — no moveSub storm.
    const before = mkAgent({ agentId: "p", subagents: subs(["coder", 3]) });
    const after = mkAgent({ agentId: "p", subagents: subs(["coder", 3], ["verifier", 2]) });
    const sb = deriveSubagents(before).specs;
    const sa = new Map(deriveSubagents(after).specs.map((s) => [s.subId, s.offset]));
    for (const s of sb) {
      expect(sa.get(s.subId)).toEqual(s.offset);
    }
  });
});

describe("PossessionController — subagent lifecycle (option b: spawn-direct, no crowd churn)", () => {
  it("parent with {coder:3} → 3 spawnSub near the parent, NO release/adopt churn", () => {
    const { env, releaseCalls, adoptCalls } = makeEnv({ idleFigures: ["builder"] });
    const ctrl = new PossessionController();
    const parent = mkAgent({ agentId: "p", currentFileId: "f1", subagents: subs(["coder", 3]) });

    const { decisions } = ctrl.reconcile([parent], env);
    const spawns = only(decisions, "spawnSub");
    expect(spawns).toHaveLength(3);
    expect(spawns.map((s) => s.subId)).toEqual([
      "p::sub::coder::0",
      "p::sub::coder::1",
      "p::sub::coder::2",
    ]);
    // Positioned at parent building + ring offset.
    expect(spawns[0].pos).toEqual({
      x: POS.f1.x + subagentRingOffset(0).x,
      y: POS.f1.y + subagentRingOffset(0).y,
    });
    // ONLY the parent itself claimed from the crowd — subagents took NO crowd
    // figure (release called once for the parent), and adopted nothing.
    expect(releaseCalls).toHaveLength(1);
    expect(adoptCalls).toHaveLength(0);
    expect(ctrl.subagentCount).toBe(3);
  });

  it("count 3 → 5 adds exactly 2 NEW omini, keeps the existing 3 (stable identity)", () => {
    const { env } = makeEnv({ idleFigures: ["builder"] });
    const ctrl = new PossessionController();
    ctrl.reconcile(
      [mkAgent({ agentId: "p", currentFileId: "f1", subagents: subs(["coder", 3]) })],
      env,
    );

    const { decisions } = ctrl.reconcile(
      [mkAgent({ agentId: "p", currentFileId: "f1", subagents: subs(["coder", 5]) })],
      env,
    );
    const spawns = only(decisions, "spawnSub");
    expect(spawns.map((s) => s.subId)).toEqual(["p::sub::coder::3", "p::sub::coder::4"]);
    expect(only(decisions, "removeSub")).toHaveLength(0);
    expect(ctrl.subagentCount).toBe(5);
  });

  it("count → 0 removes all subagents", () => {
    const { env } = makeEnv({ idleFigures: ["builder"] });
    const ctrl = new PossessionController();
    ctrl.reconcile(
      [mkAgent({ agentId: "p", currentFileId: "f1", subagents: subs(["coder", 3]) })],
      env,
    );
    const { decisions } = ctrl.reconcile(
      [mkAgent({ agentId: "p", currentFileId: "f1", subagents: [] })],
      env,
    );
    expect(only(decisions, "removeSub").map((d) => d.subId).sort()).toEqual([
      "p::sub::coder::0",
      "p::sub::coder::1",
      "p::sub::coder::2",
    ]);
    expect(ctrl.subagentCount).toBe(0);
  });

  it("parent ending removes its subagents too (and releases the parent omino)", () => {
    const { env } = makeEnv({ idleFigures: ["builder"] });
    const ctrl = new PossessionController();
    ctrl.reconcile(
      [mkAgent({ agentId: "p", currentFileId: "f1", subagents: subs(["coder", 2]) })],
      env,
    );
    const { decisions } = ctrl.reconcile([], env);
    expect(only(decisions, "release")).toHaveLength(1);
    expect(only(decisions, "removeSub")).toHaveLength(2);
    expect(ctrl.subagentCount).toBe(0);
    expect(ctrl.placedCount).toBe(0);
  });

  it("identical subagent diff repeated → no spawn/remove/move churn", () => {
    const { env } = makeEnv({ idleFigures: ["builder"] });
    const ctrl = new PossessionController();
    const p = mkAgent({ agentId: "p", currentFileId: "f1", subagents: subs(["coder", 3]) });
    ctrl.reconcile([p], env);
    const { decisions } = ctrl.reconcile([p], env);
    expect(only(decisions, "spawnSub")).toHaveLength(0);
    expect(only(decisions, "removeSub")).toHaveLength(0);
    expect(only(decisions, "moveSub")).toHaveLength(0);
  });

  it("parent changing building re-rings its existing subagents at the new building (moveSub, no churn)", () => {
    const { env } = makeEnv({ idleFigures: ["builder"] });
    const ctrl = new PossessionController();
    ctrl.reconcile(
      [mkAgent({ agentId: "p", currentFileId: "f1", subagents: subs(["coder", 2]) })],
      env,
    );

    const { decisions } = ctrl.reconcile(
      [mkAgent({ agentId: "p", currentFileId: "f2", subagents: subs(["coder", 2]) })],
      env,
    );
    // No spawn/remove — the same 2 subs, just repositioned at f2's ring.
    expect(only(decisions, "spawnSub")).toHaveLength(0);
    expect(only(decisions, "removeSub")).toHaveLength(0);
    const moves = only(decisions, "moveSub");
    expect(moves.map((m) => m.subId).sort()).toEqual([
      "p::sub::coder::0",
      "p::sub::coder::1",
    ]);
    expect(moves[0].pos).toEqual({
      x: POS.f2.x + subagentRingOffset(0).x,
      y: POS.f2.y + subagentRingOffset(0).y,
    });
    // The parent itself walked, not re-claimed.
    expect(only(decisions, "walk")).toHaveLength(1);
  });
});

// ---------------------------------------------------------------------------
// Polis-P6 — GLOBAL subagent cap (across all parents), on top of the per-parent cap.
// ---------------------------------------------------------------------------

/** An env that resolves ANY fileId to a deterministic position (so we can place
 *  an arbitrary number of parents on distinct buildings) and records release/adopt. */
function makeUnboundedEnv() {
  const releaseCalls: ReleaseCall[] = [];
  const adoptCalls: AdoptCall[] = [];
  const env: PossessionEnv = {
    resolve(fileId) {
      // Deterministic per-fileId anchor — every parent building resolves.
      const n = fileId.length + (fileId.charCodeAt(fileId.length - 1) || 0);
      return { x: n * 10, y: n * 7 };
    },
    // No crowd figures available → every parent spawns fresh (we don't care about
    // claim churn here; the cap is about derived subagent omini count).
    release() {
      releaseCalls.push({ figure: "builder", nearIso: { x: 0, y: 0 } });
      return null;
    },
    adopt(figure, pos) {
      adoptCalls.push({ figure, pos });
    },
    agentPos() {
      return null;
    },
  };
  return { env, releaseCalls, adoptCalls };
}

describe("PossessionController — GLOBAL subagent cap (Polis-P6)", () => {
  it("caps total subagent omini at MAX_SUBAGENT_OMINI_GLOBAL across many parents", () => {
    const { env } = makeUnboundedEnv();
    const ctrl = new PossessionController();
    // 10 parents × 5 coder subagents each = 50 derived omini, well past the global
    // cap (24) AND each within the per-parent cap (6).
    const parents = Array.from({ length: 10 }, (_, i) =>
      mkAgent({ agentId: `p${i}`, currentFileId: `file${i}`, subagents: subs(["coder", 5]) }),
    );
    const { decisions, droppedSubagents } = ctrl.reconcile(parents, env);
    const spawns = only(decisions, "spawnSub");
    expect(spawns).toHaveLength(MAX_SUBAGENT_OMINI_GLOBAL);
    expect(ctrl.subagentCount).toBe(MAX_SUBAGENT_OMINI_GLOBAL);
    // 50 wanted − 24 admitted = 26 dropped (none hit the per-parent cap here).
    expect(droppedSubagents).toBe(50 - MAX_SUBAGENT_OMINI_GLOBAL);
  });

  it("drops the HIGHEST-index parents/slots deterministically (stable drop rule)", () => {
    const { env } = makeUnboundedEnv();
    const ctrl = new PossessionController();
    // 6 parents × 5 = 30 wanted. Admitted in parent order then slot order, so the
    // first 24 (parents p0..p3 fully = 20, then p4 slots 0..3) are kept; p4 slot 4
    // and all of p5 are dropped.
    const parents = Array.from({ length: 6 }, (_, i) =>
      mkAgent({ agentId: `p${i}`, currentFileId: `file${i}`, subagents: subs(["coder", 5]) }),
    );
    const { decisions } = ctrl.reconcile(parents, env);
    const ids = only(decisions, "spawnSub").map((s) => s.subId);
    // p0..p3 fully present (slots 0..4).
    for (let p = 0; p <= 3; p++) {
      for (let s = 0; s <= 4; s++) expect(ids).toContain(`p${p}::sub::coder::${s}`);
    }
    // p4 keeps slots 0..3, drops slot 4.
    for (let s = 0; s <= 3; s++) expect(ids).toContain(`p4::sub::coder::${s}`);
    expect(ids).not.toContain("p4::sub::coder::4");
    // p5 is entirely dropped.
    for (let s = 0; s <= 4; s++) expect(ids).not.toContain(`p5::sub::coder::${s}`);
  });

  it("is STABLE across an identical re-reconcile — no churn at the cap boundary", () => {
    const { env } = makeUnboundedEnv();
    const ctrl = new PossessionController();
    const parents = Array.from({ length: 6 }, (_, i) =>
      mkAgent({ agentId: `p${i}`, currentFileId: `file${i}`, subagents: subs(["coder", 5]) }),
    );
    ctrl.reconcile(parents, env); // first apply
    // Re-reconcile the IDENTICAL input: the admitted set must be byte-stable, so
    // NO spawn (already placed), NO remove (the dropped ones were never placed),
    // NO move (no parent changed building). Only refresh decisions for parents.
    const { decisions } = ctrl.reconcile(parents, env);
    expect(only(decisions, "spawnSub")).toHaveLength(0);
    expect(only(decisions, "removeSub")).toHaveLength(0);
    expect(only(decisions, "moveSub")).toHaveLength(0);
    expect(ctrl.subagentCount).toBe(MAX_SUBAGENT_OMINI_GLOBAL);
  });

  it("is STABLE across MANY identical re-reconciles at the boundary — zero spawn/remove churn (#3)", () => {
    // Hardening (#3): a steady over-budget set must NOT oscillate at the global-cap
    // boundary. The over-budget subs are never placed and the admission order is
    // deterministic, so each subsequent identical diff must emit NO spawnSub and NO
    // removeSub (which would be a per-diff revive/teardown flip). Population stays
    // pinned at the cap — the over-budget tail is genuinely dropped, never placed.
    const { env } = makeUnboundedEnv();
    const ctrl = new PossessionController();
    const parents = Array.from({ length: 6 }, (_, i) =>
      mkAgent({ agentId: `p${i}`, currentFileId: `file${i}`, subagents: subs(["coder", 5]) }),
    );
    ctrl.reconcile(parents, env); // first apply (placing the admitted 24)
    for (let k = 0; k < 4; k++) {
      const { decisions } = ctrl.reconcile(parents, env);
      expect(only(decisions, "spawnSub")).toHaveLength(0);
      expect(only(decisions, "removeSub")).toHaveLength(0);
      expect(only(decisions, "moveSub")).toHaveLength(0);
      expect(ctrl.subagentCount).toBe(MAX_SUBAGENT_OMINI_GLOBAL);
    }
  });

  it("a previously-admitted sub pushed over the global cap is torn down (removeSub)", () => {
    const { env } = makeUnboundedEnv();
    const ctrl = new PossessionController();
    // Start: 4 parents × 5 = 20 omini (all fit under 24).
    const four = Array.from({ length: 4 }, (_, i) =>
      mkAgent({ agentId: `p${i}`, currentFileId: `file${i}`, subagents: subs(["coder", 5]) }),
    );
    ctrl.reconcile(four, env);
    expect(ctrl.subagentCount).toBe(20);
    // Add a 5th parent FIRST in the list with 5 omini → it pushes the budget so the
    // LAST parent's tail slots overflow and must be removed. Order: pNew first.
    const withNew = [
      mkAgent({ agentId: "pNew", currentFileId: "fileNew", subagents: subs(["coder", 5]) }),
      ...four,
    ];
    const { decisions } = ctrl.reconcile(withNew, env);
    // Total wanted = 25, cap 24 → exactly 1 omino dropped → exactly 1 removeSub of a
    // previously-admitted tail sub, and the count stays at the cap.
    expect(only(decisions, "removeSub")).toHaveLength(1);
    expect(ctrl.subagentCount).toBe(MAX_SUBAGENT_OMINI_GLOBAL);
  });
});

describe("PossessionController.clear — full reset without adopt", () => {
  it("clear() drops all tracked state and does NOT adopt (AmbientLayer.clear balances)", () => {
    const { env, adoptCalls } = makeEnv({ idleFigures: ["builder"] });
    const ctrl = new PossessionController();
    ctrl.reconcile(
      [mkAgent({ agentId: "p", currentFileId: "f1", subagents: subs(["coder", 2]) })],
      env,
    );
    ctrl.clear();
    expect(ctrl.placedCount).toBe(0);
    expect(ctrl.subagentCount).toBe(0);
    expect(adoptCalls).toHaveLength(0);
  });
});
