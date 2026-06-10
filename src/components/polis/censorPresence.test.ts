import { describe, it, expect } from "vitest";
import {
  CensorPresence,
  CENSOR_DEBOUNCE_MS,
  type CensorEnv,
  type CensorDecision,
  type ResolvedBuilding,
} from "./censorPresence";
import type { IsoPoint } from "./iso";

// Polis-P5 — the PURE CensorPresence, tested HEADLESSLY against a mock CensorEnv
// (no PIXI, no real clock, no Math.random/Date.now). The env records every
// release/adopt so the claimedCount contract (one adopt per claim, none for a
// fallback spawn-fresh) can be asserted exactly. The debounce clock is injected
// (nowMs), so a burst + settle is fully deterministic.

// ---------------------------------------------------------------------------
// Test world + mock env
// ---------------------------------------------------------------------------

// relPath → resolved building (fileId + iso). Two buildings on the map.
const WORLD: Record<string, ResolvedBuilding> = {
  "src/a.ts": { fileId: "fa", iso: { x: 100, y: 50 } },
  "src/b.ts": { fileId: "fb", iso: { x: 200, y: 80 } },
};

interface ReleaseCall {
  nearIso: IsoPoint;
}
interface AdoptCall {
  pos: IsoPoint;
}

/**
 * A controllable mock env. `idleAvailable` controls whether `release` hands off a
 * firefighter (claim-from-crowd) or returns null (→ spawn-fresh). `firefighterAt`
 * feeds `firefighterPos` (the adopt position).
 */
function makeEnv(opts?: { idleAvailable?: boolean; firefighterAt?: IsoPoint | null }) {
  let idleAvailable = opts?.idleAvailable ?? true;
  let firefighterAt: IsoPoint | null = opts?.firefighterAt ?? null;
  const releaseCalls: ReleaseCall[] = [];
  const adoptCalls: AdoptCall[] = [];
  let handoffSeq = 0;

  const env: CensorEnv = {
    resolveRelPath(relPath) {
      return WORLD[relPath] ?? null;
    },
    release(nearIso) {
      releaseCalls.push({ nearIso });
      if (!idleAvailable) return null;
      handoffSeq += 1;
      return {
        pos: { x: nearIso.x - 10, y: nearIso.y - 10 },
        nodeId: `node${handoffSeq}`,
      };
    },
    adopt(pos) {
      adoptCalls.push({ pos });
    },
    firefighterPos() {
      return firefighterAt;
    },
  };

  return {
    env,
    releaseCalls,
    adoptCalls,
    setIdle: (v: boolean) => {
      idleAvailable = v;
    },
    setFirefighterAt: (p: IsoPoint | null) => {
      firefighterAt = p;
    },
  };
}

function only<K extends CensorDecision["kind"]>(
  decisions: CensorDecision[],
  kind: K,
): Extract<CensorDecision, { kind: K }>[] {
  return decisions.filter((d) => d.kind === kind) as Extract<
    CensorDecision,
    { kind: K }
  >[];
}

/** Drive a full naming event → debounce → flush, returning the flush decisions. */
function reviewFlush(
  c: CensorPresence,
  env: CensorEnv,
  files: string[],
  startMs: number,
  projectId = "p1",
): CensorDecision[] {
  const onNow = c.onFindings({ projectId, files }, startMs, env);
  expect(onNow).toEqual([]); // naming events are debounced, never immediate
  return c.tick(startMs + CENSOR_DEBOUNCE_MS, env);
}

// ---------------------------------------------------------------------------
// Claim + walk + extinguishing
// ---------------------------------------------------------------------------

describe("CensorPresence — claim + walk + extinguishing", () => {
  it("event with a resolvable file + gemma online → CLAIM + walk + extinguishing", () => {
    const { env, releaseCalls, adoptCalls } = makeEnv({ idleAvailable: true });
    const c = new CensorPresence();
    c.setGemmaStatus("available", env);

    const decisions = reviewFlush(c, env, ["src/a.ts"], 1000);

    const claims = only(decisions, "createClaimed");
    expect(claims).toHaveLength(1);
    expect(claims[0].targetFileId).toBe("fa");
    expect(claims[0].targetIso).toEqual(WORLD["src/a.ts"].iso);
    // release asked near the target building; no adopt yet.
    expect(releaseCalls).toEqual([{ nearIso: WORLD["src/a.ts"].iso }]);
    expect(adoptCalls).toEqual([]);
    // The water-arc tell turned on.
    const ext = only(decisions, "extinguishing");
    expect(ext).toEqual([{ kind: "extinguishing", on: true }]);
    expect(c.placed).toBe(true);
    expect(c.origin).toBe("claimed-from-crowd");
    expect(c.extinguishing).toBe(true);
    expect(c.fileId).toBe("fa");
  });

  it("treats 'unknown' gemma (not yet probed) as NOT offline → still reacts", () => {
    const { env } = makeEnv({ idleAvailable: true });
    const c = new CensorPresence();
    // No setGemmaStatus call → default "unknown".
    const decisions = reviewFlush(c, env, ["src/a.ts"], 1000);
    expect(only(decisions, "createClaimed")).toHaveLength(1);
  });
});

// ---------------------------------------------------------------------------
// Unresolvable files
// ---------------------------------------------------------------------------

describe("CensorPresence — unresolvable files are dropped", () => {
  it("event with ONLY unresolvable files → no claim (nothing fabricated)", () => {
    const { env, releaseCalls } = makeEnv({ idleAvailable: true });
    const c = new CensorPresence();
    c.setGemmaStatus("available", env);

    const onNow = c.onFindings(
      { projectId: "p1", files: ["nope/x.ts", "also/missing.ts"] },
      1000,
      env,
    );
    expect(onNow).toEqual([]);
    // No pending was armed → nothing to flush.
    expect(c.hasPending).toBe(false);
    const flush = c.tick(1000 + CENSOR_DEBOUNCE_MS, env);
    expect(flush).toEqual([]);
    expect(c.placed).toBe(false);
    expect(releaseCalls).toEqual([]);
  });

  it("picks the FIRST resolvable file when some are unresolvable", () => {
    const { env } = makeEnv({ idleAvailable: true });
    const c = new CensorPresence();
    c.setGemmaStatus("available", env);
    // First entry is unresolvable; src/b.ts is the first resolvable → target fb.
    const decisions = reviewFlush(c, env, ["gone/x.ts", "src/b.ts", "src/a.ts"], 1000);
    const claims = only(decisions, "createClaimed");
    expect(claims).toHaveLength(1);
    expect(claims[0].targetFileId).toBe("fb");
  });
});

// ---------------------------------------------------------------------------
// Gemma offline suppression
// ---------------------------------------------------------------------------

describe("CensorPresence — gemma offline", () => {
  it("gemma offline → no omino even with events", () => {
    const { env, releaseCalls } = makeEnv({ idleAvailable: true });
    const c = new CensorPresence();
    c.setGemmaStatus("offline", env);

    const onNow = c.onFindings({ projectId: "p1", files: ["src/a.ts"] }, 1000, env);
    expect(onNow).toEqual([]);
    expect(c.hasPending).toBe(false);
    expect(c.tick(1000 + CENSOR_DEBOUNCE_MS, env)).toEqual([]);
    expect(c.placed).toBe(false);
    expect(releaseCalls).toEqual([]);
  });

  it("if a firefighter is already present, gemma→offline RELEASES it (one adopt)", () => {
    const { env, adoptCalls, setFirefighterAt } = makeEnv({ idleAvailable: true });
    const c = new CensorPresence();
    c.setGemmaStatus("available", env);
    reviewFlush(c, env, ["src/a.ts"], 1000);
    expect(c.placed).toBe(true);
    setFirefighterAt({ x: 120, y: 60 });

    const decisions = c.setGemmaStatus("offline", env);

    expect(c.placed).toBe(false);
    expect(c.extinguishing).toBe(false);
    // extinguishing off, then destroy.
    expect(only(decisions, "extinguishing")).toEqual([
      { kind: "extinguishing", on: false },
    ]);
    expect(only(decisions, "destroy")).toHaveLength(1);
    // EXACTLY one adopt per claim, at the firefighter's last position.
    expect(adoptCalls).toEqual([{ pos: { x: 120, y: 60 } }]);
  });
});

// ---------------------------------------------------------------------------
// Settle (empty-files) — stop extinguishing + adopt
// ---------------------------------------------------------------------------

describe("CensorPresence — settle event", () => {
  it("empty-files settle → stop extinguishing + adopt (one adopt per claim)", () => {
    const { env, adoptCalls, setFirefighterAt } = makeEnv({ idleAvailable: true });
    const c = new CensorPresence();
    c.setGemmaStatus("available", env);
    reviewFlush(c, env, ["src/a.ts"], 1000);
    setFirefighterAt({ x: 100, y: 50 });

    // The empty-files settle is applied IMMEDIATELY (not debounced).
    const decisions = c.onFindings({ projectId: "p1", files: [] }, 2000, env);

    expect(only(decisions, "extinguishing")).toEqual([
      { kind: "extinguishing", on: false },
    ]);
    expect(only(decisions, "destroy")).toHaveLength(1);
    expect(adoptCalls).toHaveLength(1);
    expect(c.placed).toBe(false);
  });

  it("a settle with NO firefighter present is a no-op (no adopt)", () => {
    const { env, adoptCalls } = makeEnv({ idleAvailable: true });
    const c = new CensorPresence();
    c.setGemmaStatus("available", env);
    const decisions = c.onFindings({ projectId: "p1", files: [] }, 1000, env);
    expect(decisions).toEqual([]);
    expect(adoptCalls).toEqual([]);
  });
});

// ---------------------------------------------------------------------------
// Walk to a different building — no re-claim
// ---------------------------------------------------------------------------

describe("CensorPresence — consecutive different buildings", () => {
  it("a DIFFERENT building → WALKS the existing firefighter (no second claim)", () => {
    const { env, releaseCalls, adoptCalls } = makeEnv({ idleAvailable: true });
    const c = new CensorPresence();
    c.setGemmaStatus("available", env);

    reviewFlush(c, env, ["src/a.ts"], 1000);
    expect(releaseCalls).toHaveLength(1);

    const decisions = reviewFlush(c, env, ["src/b.ts"], 5000);

    // No second claim, no adopt, no new create — just a walk to fb.
    expect(releaseCalls).toHaveLength(1);
    expect(adoptCalls).toHaveLength(0);
    expect(only(decisions, "createClaimed")).toHaveLength(0);
    expect(only(decisions, "createFresh")).toHaveLength(0);
    const walks = only(decisions, "walk");
    expect(walks).toHaveLength(1);
    expect(walks[0].targetFileId).toBe("fb");
    expect(c.fileId).toBe("fb");
    // It was already extinguishing → no redundant re-toggle.
    expect(only(decisions, "extinguishing")).toEqual([]);
  });

  it("the SAME building again → no walk, no re-claim (idempotent)", () => {
    const { env, releaseCalls, adoptCalls } = makeEnv({ idleAvailable: true });
    const c = new CensorPresence();
    c.setGemmaStatus("available", env);
    reviewFlush(c, env, ["src/a.ts"], 1000);
    const decisions = reviewFlush(c, env, ["src/a.ts"], 5000);
    expect(releaseCalls).toHaveLength(1);
    expect(adoptCalls).toHaveLength(0);
    expect(only(decisions, "walk")).toHaveLength(0);
    expect(only(decisions, "createClaimed")).toHaveLength(0);
  });
});

// ---------------------------------------------------------------------------
// Fallback spawn-fresh — NO adopt on settle
// ---------------------------------------------------------------------------

describe("CensorPresence — spawn-fresh fallback", () => {
  it("no idle firefighter → spawn-fresh; settle does NOT adopt", () => {
    const { env, releaseCalls, adoptCalls } = makeEnv({ idleAvailable: false });
    const c = new CensorPresence();
    c.setGemmaStatus("available", env);

    const decisions = reviewFlush(c, env, ["src/a.ts"], 1000);
    // release was attempted (returned null) → spawn-fresh.
    expect(releaseCalls).toHaveLength(1);
    expect(only(decisions, "createFresh")).toHaveLength(1);
    expect(only(decisions, "createClaimed")).toHaveLength(0);
    expect(c.origin).toBe("spawned-fresh");

    // Settle: stop extinguishing + destroy, but NEVER adopt (took no crowd figure).
    const settle = c.onFindings({ projectId: "p1", files: [] }, 2000, env);
    expect(only(settle, "destroy")).toHaveLength(1);
    expect(adoptCalls).toEqual([]);
  });
});

// ---------------------------------------------------------------------------
// Debounce — a burst is coalesced into ONE claim, no flicker
// ---------------------------------------------------------------------------

describe("CensorPresence — debounce", () => {
  it("a burst of events within the window → ONE claim, no flicker", () => {
    const { env, releaseCalls } = makeEnv({ idleAvailable: true });
    const c = new CensorPresence();
    c.setGemmaStatus("available", env);

    // Three events in quick succession (well within CENSOR_DEBOUNCE_MS).
    expect(c.onFindings({ projectId: "p1", files: ["src/a.ts"] }, 1000, env)).toEqual([]);
    expect(c.onFindings({ projectId: "p1", files: ["src/a.ts"] }, 1100, env)).toEqual([]);
    expect(c.onFindings({ projectId: "p1", files: ["src/b.ts"] }, 1200, env)).toEqual([]);
    // Ticking BEFORE the (refreshed) deadline does nothing.
    expect(c.tick(1300, env)).toEqual([]);
    expect(releaseCalls).toHaveLength(0);

    // Deadline is keyed off the LAST event (1200 + window).
    const flush = c.tick(1200 + CENSOR_DEBOUNCE_MS, env);
    expect(releaseCalls).toHaveLength(1); // exactly one claim
    const claims = only(flush, "createClaimed");
    expect(claims).toHaveLength(1);
    // The last event in the burst won the target (deterministic last-write).
    expect(claims[0].targetFileId).toBe("fb");
    // A second tick is idempotent (pending already cleared).
    expect(c.tick(9999, env)).toEqual([]);
  });

  it("each new event REFRESHES the debounce deadline", () => {
    const { env } = makeEnv({ idleAvailable: true });
    const c = new CensorPresence();
    c.setGemmaStatus("available", env);
    c.onFindings({ projectId: "p1", files: ["src/a.ts"] }, 1000, env);
    expect(c.pendingDeadlineMs).toBe(1000 + CENSOR_DEBOUNCE_MS);
    c.onFindings({ projectId: "p1", files: ["src/a.ts"] }, 1400, env);
    expect(c.pendingDeadlineMs).toBe(1400 + CENSOR_DEBOUNCE_MS);
  });
});

// ---------------------------------------------------------------------------
// Project switch — release the old firefighter, keep gemma intact
// ---------------------------------------------------------------------------

describe("CensorPresence — releaseForSwitch", () => {
  it("releases the firefighter (one adopt) without changing gemma status", () => {
    const { env, adoptCalls, setFirefighterAt } = makeEnv({ idleAvailable: true });
    const c = new CensorPresence();
    c.setGemmaStatus("available", env);
    reviewFlush(c, env, ["src/a.ts"], 1000);
    setFirefighterAt({ x: 90, y: 40 });

    const decisions = c.releaseForSwitch(env);
    expect(only(decisions, "destroy")).toHaveLength(1);
    expect(adoptCalls).toEqual([{ pos: { x: 90, y: 40 } }]);
    expect(c.placed).toBe(false);

    // gemma is still "available" → a new event reacts immediately (not suppressed).
    const next = reviewFlush(c, env, ["src/b.ts"], 5000, "p2");
    expect(only(next, "createClaimed")).toHaveLength(1);
  });
});

// ---------------------------------------------------------------------------
// Determinism
// ---------------------------------------------------------------------------

describe("CensorPresence — determinism", () => {
  it("the same event sequence + clock yields the same decisions", () => {
    const run = () => {
      const { env } = makeEnv({ idleAvailable: true });
      const c = new CensorPresence();
      c.setGemmaStatus("available", env);
      const out: CensorDecision[] = [];
      out.push(...reviewFlush(c, env, ["src/a.ts"], 1000));
      out.push(...reviewFlush(c, env, ["src/b.ts"], 5000));
      out.push(...c.onFindings({ projectId: "p1", files: [] }, 9000, env));
      return out;
    };
    expect(run()).toEqual(run());
  });
});
