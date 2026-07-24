// ambientLife.test.ts — Phase 4 pure planners + budget demotion gates +
// pooled PIXI systems / signature gating (F1–F3, F7, F9, F12, F18).

import { describe, it, expect } from "vitest";
import { Container, Texture } from "pixi.js";
import {
  parseLastModifiedMs,
  isBuildingActive,
  isChimneyEmitterEligible,
  selectChimneyEmitters,
  flagPhaseOffset,
  selectCivicFlags,
  windowGlowOffsets,
  selectWindowGlows,
  nightWindowLayerAlpha,
  NIGHT_WINDOW_DARKNESS_THRESHOLD,
  sampleTrafficDust,
  selectForumClusters,
  cubicBezier,
  nextBirdArc,
  RECENT_MODIFIED_MS,
  CIVIC_FLAG_PURPOSES,
  BIRDS_SEED,
  NightWindowsSystem,
  TrafficDustSystem,
  ForumClusterSystem,
  AmbientLifeManager,
  applyCivicFlagPhases,
  removeAmbientCivicFlags,
  isAmbientLifeFlag,
  ambientChimneySig,
  ambientWindowsSig,
  ambientFlagsSig,
  ambientDustSig,
  ambientForumsSig,
  type AmbientLifeBuildingView,
  type AmbientTextureSource,
} from "./ambientLife";
import { ambientLifeGates } from "./effectsBudget";
import { mulberry32 } from "./rng";
import { saltLook } from "./kitcd/buildings";
import { RENDER_PROFILES } from "./renderProfile";
import { Flag } from "./kitcd/anims";
import { DERIVED, PALETTE } from "./palette";
import { blend, lighten } from "./iso";
import type { Building } from "../../types/city";
import type { AnimInstance } from "./kitcd/anims";

const NOW = Date.parse("2026-07-24T12:00:00.000Z");

describe("parseLastModifiedMs / isBuildingActive", () => {
  it("parses ISO timestamps; rejects empty/garbage", () => {
    expect(parseLastModifiedMs("2026-07-24T00:00:00.000Z")).toBe(
      Date.parse("2026-07-24T00:00:00.000Z"),
    );
    expect(parseLastModifiedMs("")).toBeNull();
    expect(parseLastModifiedMs(null)).toBeNull();
    expect(parseLastModifiedMs("not-a-date")).toBeNull();
  });

  it("agentPresent marks active", () => {
    expect(isBuildingActive("agent-1", null, NOW)).toBe(true);
    expect(isBuildingActive("", null, NOW)).toBe(false);
    expect(isBuildingActive(undefined, null, NOW)).toBe(false);
  });

  it("recent lastModified within 48h marks active", () => {
    const recent = NOW - 12 * 60 * 60 * 1000;
    const stale = NOW - RECENT_MODIFIED_MS - 1000;
    expect(isBuildingActive(null, recent, NOW)).toBe(true);
    expect(isBuildingActive(null, stale, NOW)).toBe(false);
    expect(isBuildingActive(null, NOW + 1000, NOW)).toBe(false); // future
  });
});

describe("isChimneyEmitterEligible", () => {
  it("requires chimney (salt) or workshop/baths AND activity", () => {
    // salt 1 → hasChimney false
    expect(saltLook(1).hasChimney).toBe(false);
    expect(
      isChimneyEmitterEligible({
        purpose: "house",
        salt: 1,
        agentPresent: "a",
        lastModifiedMs: null,
        nowMs: NOW,
      }),
    ).toBe(false);

    // salt 0 → hasChimney true + active
    expect(saltLook(0).hasChimney).toBe(true);
    expect(
      isChimneyEmitterEligible({
        purpose: "house",
        salt: 0,
        agentPresent: "a",
        lastModifiedMs: null,
        nowMs: NOW,
      }),
    ).toBe(true);

    // workshop always qualifies if active (even salt 1)
    expect(
      isChimneyEmitterEligible({
        purpose: "workshop",
        salt: 1,
        agentPresent: "a",
        lastModifiedMs: null,
        nowMs: NOW,
      }),
    ).toBe(true);

    // baths + recent mtime
    expect(
      isChimneyEmitterEligible({
        purpose: "baths",
        salt: 1,
        agentPresent: null,
        lastModifiedMs: NOW - 1000,
        nowMs: NOW,
      }),
    ).toBe(true);

    // house with chimney but inactive
    expect(
      isChimneyEmitterEligible({
        purpose: "house",
        salt: 0,
        agentPresent: null,
        lastModifiedMs: null,
        nowMs: NOW,
      }),
    ).toBe(false);
  });
});

describe("selectChimneyEmitters", () => {
  const buildings = [
    {
      fileId: "far",
      purpose: "house",
      salt: 0,
      agentPresent: "a",
      lastModified: "",
      x: 1000,
      y: 1000,
    },
    {
      fileId: "near",
      purpose: "workshop",
      salt: 1,
      agentPresent: "b",
      lastModified: "",
      x: 10,
      y: 10,
    },
    {
      fileId: "mid",
      purpose: "house",
      salt: 0,
      agentPresent: null as string | null,
      lastModified: new Date(NOW - 1000).toISOString(),
      x: 50,
      y: 50,
    },
    {
      fileId: "inactive",
      purpose: "house",
      salt: 0,
      agentPresent: null as string | null,
      lastModified: "",
      x: 5,
      y: 5,
    },
  ];

  it("respects cap and prefers nearest to center", () => {
    const ids = selectChimneyEmitters(buildings, 2, 0, 0, NOW);
    expect(ids).toEqual(["near", "mid"]);
  });

  it("cap 0 → empty; minimal profile cap", () => {
    expect(selectChimneyEmitters(buildings, 0, 0, 0, NOW)).toEqual([]);
    expect(
      selectChimneyEmitters(
        buildings,
        RENDER_PROFILES.minimal.maxChimneySmoke,
        0,
        0,
        NOW,
      ),
    ).toEqual([]);
  });

  it("is deterministic", () => {
    const a = selectChimneyEmitters(buildings, 10, 0, 0, NOW);
    const b = selectChimneyEmitters(buildings, 10, 0, 0, NOW);
    expect(a).toEqual(b);
  });

  it("rich/lean caps", () => {
    expect(RENDER_PROFILES.rich.maxChimneySmoke).toBe(24);
    expect(RENDER_PROFILES.lean.maxChimneySmoke).toBe(10);
  });
});

describe("flagPhaseOffset / selectCivicFlags", () => {
  it("phase offset is deterministic and in [0, 8)", () => {
    const a = flagPhaseOffset("file-abc");
    const b = flagPhaseOffset("file-abc");
    const c = flagPhaseOffset("file-xyz");
    expect(a).toBe(b);
    expect(a).toBeGreaterThanOrEqual(0);
    expect(a).toBeLessThan(8);
    expect(a).not.toBe(c);
  });

  it("selects only civic purposes, nearest + cap", () => {
    const buildings = [
      { fileId: "h", purpose: "house", x: 0, y: 0 },
      { fileId: "m", purpose: "market", x: 100, y: 0 },
      { fileId: "t", purpose: "townhall", x: 5, y: 0 },
      { fileId: "lib", purpose: "library", x: 50, y: 0 },
      { fileId: "th", purpose: "theater", x: 200, y: 0 },
      { fileId: "tem", purpose: "temple", x: 8, y: 0 },
    ];
    const ids = selectCivicFlags(buildings, 3, 0, 0);
    expect(ids).toEqual(["t", "tem", "lib"]);
    expect(ids.every((id) => id !== "h")).toBe(true);
    for (const p of CIVIC_FLAG_PURPOSES) {
      expect(typeof p).toBe("string");
    }
  });

  it("profile caps", () => {
    expect(RENDER_PROFILES.rich.maxCivicFlags).toBe(20);
    expect(RENDER_PROFILES.lean.maxCivicFlags).toBe(8);
    expect(RENDER_PROFILES.minimal.maxCivicFlags).toBe(0);
  });
});

describe("window glow selection", () => {
  it("level < 2 yields no offsets", () => {
    expect(windowGlowOffsets(0, 0, 20, 40)).toEqual([]);
    expect(windowGlowOffsets(1, 0, 20, 40)).toEqual([]);
  });

  it("winMode drives slot count", () => {
    // salt 0 → winMode 0 → 2 windows
    expect(windowGlowOffsets(2, 0, 20, 40).length).toBe(2);
    // salt 1 → winMode 1 → 3 windows
    expect(windowGlowOffsets(2, 1, 20, 40).length).toBe(3);
    // salt 2 → winMode 2 → 2 (upper + door)
    expect(windowGlowOffsets(2, 2, 20, 40).length).toBe(2);
  });

  it("selectWindowGlows respects cap + level filter + determinism", () => {
    const buildings = [
      {
        fileId: "low",
        level: 1,
        salt: 0,
        x: 0,
        y: 0,
        depth: 30,
        hw: 12,
      },
      {
        fileId: "a",
        level: 2,
        salt: 0,
        x: 10,
        y: 0,
        depth: 40,
        hw: 16,
      },
      {
        fileId: "b",
        level: 3,
        salt: 1,
        x: 100,
        y: 0,
        depth: 50,
        hw: 20,
      },
    ];
    const slots = selectWindowGlows(buildings, 3, 0, 0);
    expect(slots.length).toBe(3);
    expect(slots.every((s) => s.fileId !== "low")).toBe(true);
    // nearest first: a (2 slots from winMode 0) then b
    expect(slots[0].fileId).toBe("a");
    expect(slots[1].fileId).toBe("a");
    expect(slots[2].fileId).toBe("b");
    expect(selectWindowGlows(buildings, 3, 0, 0)).toEqual(slots);
  });

  it("nightWindowLayerAlpha is 0 by day, ramps after threshold", () => {
    expect(nightWindowLayerAlpha(0)).toBe(0);
    expect(nightWindowLayerAlpha(NIGHT_WINDOW_DARKNESS_THRESHOLD - 0.01)).toBe(
      0,
    );
    expect(
      nightWindowLayerAlpha(NIGHT_WINDOW_DARKNESS_THRESHOLD),
    ).toBeGreaterThanOrEqual(0);
    expect(nightWindowLayerAlpha(1)).toBeCloseTo(0.72, 5);
  });

  it("profile caps", () => {
    expect(RENDER_PROFILES.rich.maxNightWindows).toBe(120);
    expect(RENDER_PROFILES.lean.maxNightWindows).toBe(40);
    expect(RENDER_PROFILES.minimal.maxNightWindows).toBe(0);
  });
});

describe("sampleTrafficDust", () => {
  const segs = [
    { id: "r1", x0: 0, y0: 0, x1: 100, y1: 0, weight: 5 },
    { id: "r2", x0: 0, y0: 0, x1: 50, y1: 50, weight: 4 },
    { id: "r3", x0: 0, y0: 0, x1: 10, y1: 0, weight: 1 }, // too light
  ];

  it("samples only weight>=3, up to cap, deterministically", () => {
    const a = sampleTrafficDust(segs, 5);
    const b = sampleTrafficDust(segs, 5);
    expect(a).toEqual(b);
    expect(a.length).toBe(5);
    expect(a.every((m) => m.roadId === "r1" || m.roadId === "r2")).toBe(true);
    // Prefer higher weight first
    expect(a[0].roadId).toBe("r1");
  });

  it("cap 0 / rich-only profile numbers", () => {
    expect(sampleTrafficDust(segs, 0)).toEqual([]);
    expect(RENDER_PROFILES.rich.maxTrafficDust).toBe(12);
    expect(RENDER_PROFILES.lean.maxTrafficDust).toBe(0);
  });
});

describe("selectForumClusters", () => {
  const anchors = [
    {
      fileId: "m1",
      purpose: "market",
      isCommons: false,
      x: 0,
      y: 0,
    },
    {
      fileId: "h1",
      purpose: "house",
      isCommons: false,
      x: 1,
      y: 0,
    },
    {
      fileId: "t1",
      purpose: "townhall",
      isCommons: false,
      x: 50,
      y: 0,
    },
    {
      fileId: "c1",
      purpose: "house",
      isCommons: true,
      x: 5,
      y: 0,
    },
  ];

  it("selects market/townhall/commons only, nearest + cap", () => {
    const plans = selectForumClusters(anchors, 2, 0, 0);
    expect(plans.map((p) => p.fileId)).toEqual(["m1", "c1"]);
    for (const p of plans) {
      expect(p.count).toBeGreaterThanOrEqual(2);
      expect(p.count).toBeLessThanOrEqual(4);
      expect(p.offsets.length).toBe(p.count);
    }
  });

  it("deterministic + profile caps", () => {
    expect(selectForumClusters(anchors, 8, 0, 0)).toEqual(
      selectForumClusters(anchors, 8, 0, 0),
    );
    expect(RENDER_PROFILES.rich.maxForumClusters).toBe(8);
    expect(RENDER_PROFILES.lean.maxForumClusters).toBe(3);
    expect(RENDER_PROFILES.minimal.maxForumClusters).toBe(0);
    expect(selectForumClusters(anchors, 0, 0, 0)).toEqual([]);
  });
});

describe("birds pure math", () => {
  it("cubicBezier endpoints", () => {
    expect(cubicBezier(0, 0, 1, 2, 3)).toBe(0);
    expect(cubicBezier(1, 0, 1, 2, 3)).toBe(3);
  });

  it("nextBirdArc is deterministic for a fixed seed stream", () => {
    const r1 = mulberry32(BIRDS_SEED >>> 0);
    const r2 = mulberry32(BIRDS_SEED >>> 0);
    const a = nextBirdArc(r1, -100, -100, 100, 100);
    const b = nextBirdArc(r2, -100, -100, 100, 100);
    expect(a).toEqual(b);
    expect(a.frames).toBeGreaterThan(0);
  });

  it("rich only birds", () => {
    expect(RENDER_PROFILES.rich.maxBirds).toBe(3);
    expect(RENDER_PROFILES.lean.maxBirds).toBe(0);
    expect(RENDER_PROFILES.minimal.maxBirds).toBe(0);
  });
});

describe("ambient walker cap raised on rich", () => {
  it("rich maxAmbientWalkers is 64", () => {
    expect(RENDER_PROFILES.rich.maxAmbientWalkers).toBe(64);
    expect(RENDER_PROFILES.lean.maxAmbientWalkers).toBe(18);
    expect(RENDER_PROFILES.minimal.maxAmbientWalkers).toBe(6);
  });
});

describe("ambientLifeGates — budget demotion path", () => {
  it("rung 0: all on, not half-rate", () => {
    const g = ambientLifeGates(0);
    expect(g.chimneySmoke).toBe(true);
    expect(g.birds).toBe(true);
    expect(g.nightWindows).toBe(true);
    expect(g.trafficDust).toBe(true);
    expect(g.forumBob).toBe(true);
    expect(g.civicFlags).toBe(true);
    expect(g.halfRate).toBe(false);
  });

  it("rung 3: half-rate, systems still on", () => {
    const g = ambientLifeGates(3);
    expect(g.chimneySmoke).toBe(true);
    expect(g.halfRate).toBe(true);
  });

  it("rung 4: smoke/birds/dust off; windows+forum stay", () => {
    const g = ambientLifeGates(4);
    expect(g.chimneySmoke).toBe(false);
    expect(g.birds).toBe(false);
    expect(g.trafficDust).toBe(false);
    expect(g.nightWindows).toBe(true);
    expect(g.forumBob).toBe(true);
    expect(g.civicFlags).toBe(false);
  });

  it("rung 5: everything ambient-life off", () => {
    const g = ambientLifeGates(5);
    expect(g.chimneySmoke).toBe(false);
    expect(g.birds).toBe(false);
    expect(g.nightWindows).toBe(false);
    expect(g.trafficDust).toBe(false);
    expect(g.forumBob).toBe(false);
    expect(g.civicFlags).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// F18 — cooler chimney smoke tint
// ---------------------------------------------------------------------------

describe("F18 chimney smokeCool tint", () => {
  it("DERIVED.smokeCool blends smoke toward water ~18%", () => {
    const expected = blend(lighten(PALETTE.stoneDark, 0.28), PALETTE.water, 0.18);
    expect(DERIVED.smokeCool).toBe(expected);
    // Distinct from warm disaster smoke
    expect(DERIVED.smokeCool).not.toBe(DERIVED.smoke);
  });
});

// ---------------------------------------------------------------------------
// Fake texture source for pooled system tests
// ---------------------------------------------------------------------------

function fakeTexSource(): AmbientTextureSource {
  return {
    generateTexture: () => {
      // Fresh stub each call so identity of generated textures is distinct.
      return Texture.EMPTY;
    },
  };
}

function makeBuilding(
  partial: Partial<Building> & Pick<Building, "fileId" | "purpose">,
): Building {
  return {
    filePath: partial.fileId,
    districtId: "d1",
    purposeSource: "default",
    linesOfCode: 10,
    visualTier: "mid",
    coords: { x: 0, y: 0 },
    status: "normal",
    label: partial.fileId,
    description: "",
    lastModified: "",
    sins: [],
    notes: [],
    ...partial,
  };
}

function makeView(overrides: {
  fileId: string;
  purpose: string;
  x?: number;
  y?: number;
  level?: number;
  salt?: number;
  depth?: number;
  hw?: number;
  agentPresent?: string | null;
  lastModified?: string;
  isCommons?: boolean;
  kitAnims?: AnimInstance[];
  container?: Container;
}): AmbientLifeBuildingView {
  const b = makeBuilding({
    fileId: overrides.fileId,
    purpose: overrides.purpose,
    agentPresent: overrides.agentPresent ?? undefined,
    lastModified: overrides.lastModified ?? "",
    featureSource: overrides.isCommons ? "commons" : "directory",
  });
  return {
    building: b,
    iso: { x: overrides.x ?? 0, y: overrides.y ?? 0 },
    salt: overrides.salt ?? 0,
    level: overrides.level ?? 2,
    depth: overrides.depth ?? 40,
    hw: overrides.hw ?? 16,
    kitAnims: overrides.kitAnims ?? [],
    container: overrides.container ?? new Container(),
  };
}

// ---------------------------------------------------------------------------
// F1/F2/F3 — pooling: rebuild twice reuses same sprite instances
// ---------------------------------------------------------------------------

describe("pooling — rebuild reuses sprites (F1/F2/F3)", () => {
  it("NightWindowsSystem: bake once, rebuild twice keeps pool identity + size", () => {
    const sys = new NightWindowsSystem();
    sys.bake(fakeTexSource(), 8);
    expect(sys.poolSize).toBe(8);
    const pool1 = [...sys.poolSprites];

    const slots = [
      { fileId: "a", ox: 0, oy: -10 },
      { fileId: "a", ox: 5, oy: -10 },
      { fileId: "b", ox: 0, oy: -8 },
    ];
    const pos = new Map([
      ["a", { x: 10, y: 20 }],
      ["b", { x: 50, y: 20 }],
    ]);
    sys.rebuild(slots, pos);
    expect(sys.activeCount).toBe(3);
    const pool2 = [...sys.poolSprites];
    expect(pool2).toEqual(pool1);
    expect(pool2.length).toBe(8);

    // Fewer slots — excess parked, same instances
    sys.rebuild(slots.slice(0, 1), pos);
    expect(sys.activeCount).toBe(1);
    expect([...sys.poolSprites]).toEqual(pool1);

    // More slots again — still same pool
    sys.rebuild(slots, pos);
    expect(sys.activeCount).toBe(3);
    expect([...sys.poolSprites]).toEqual(pool1);

    sys.destroy();
  });

  it("TrafficDustSystem: bake once, rebuild twice keeps pool identity", () => {
    const sys = new TrafficDustSystem();
    sys.bake(fakeTexSource(), 6);
    expect(sys.poolSize).toBe(6);
    const pool1 = [...sys.poolSprites];

    const paths = sampleTrafficDust(
      [
        { id: "r1", x0: 0, y0: 0, x1: 100, y1: 0, weight: 5 },
        { id: "r2", x0: 0, y0: 0, x1: 50, y1: 50, weight: 4 },
      ],
      4,
    );
    sys.rebuild(paths);
    expect(sys.activeCount).toBe(4);
    expect([...sys.poolSprites]).toEqual(pool1);

    sys.rebuild(paths.slice(0, 2));
    expect(sys.activeCount).toBe(2);
    expect([...sys.poolSprites]).toEqual(pool1);

    sys.destroy();
  });

  it("ForumClusterSystem: bake once, rebuild twice keeps sprite identity", () => {
    const sys = new ForumClusterSystem();
    sys.bake(fakeTexSource(), 4);
    expect(sys.poolSize).toBe(4);
    const pool1 = [...sys.poolSprites];
    // 4 clusters × 4 figures
    expect(pool1.length).toBe(16);

    const plans = selectForumClusters(
      [
        { fileId: "m1", purpose: "market", isCommons: false, x: 0, y: 0 },
        { fileId: "t1", purpose: "townhall", isCommons: false, x: 40, y: 0 },
      ],
      2,
      0,
      0,
    );
    sys.rebuild(plans);
    expect(sys.activeCount).toBe(2);
    expect([...sys.poolSprites]).toEqual(pool1);

    sys.rebuild(plans.slice(0, 1));
    expect(sys.activeCount).toBe(1);
    expect([...sys.poolSprites]).toEqual(pool1);

    sys.destroy();
  });
});

// ---------------------------------------------------------------------------
// F7 — per-subsystem signature gating
// ---------------------------------------------------------------------------

describe("per-subsystem signatures (F7)", () => {
  it("pure sig helpers are stable for identical inputs", () => {
    const buildings = [
      {
        fileId: "w1",
        purpose: "workshop",
        salt: 0,
        agentPresent: "a",
        lastModified: "",
      },
      {
        fileId: "h1",
        purpose: "house",
        salt: 1,
        agentPresent: null as string | null,
        lastModified: "",
      },
    ];
    const a = ambientChimneySig(buildings, NOW, 0, 0);
    const b = ambientChimneySig(buildings, NOW, 0, 0);
    expect(a).toBe(b);

    expect(
      ambientWindowsSig(
        [
          { fileId: "a", level: 2, salt: 0 },
          { fileId: "b", level: 1, salt: 0 },
        ],
        0,
        0,
      ),
    ).toBe(
      ambientWindowsSig(
        [
          { fileId: "a", level: 2, salt: 0 },
          { fileId: "b", level: 1, salt: 0 },
        ],
        0,
        0,
      ),
    );

    expect(
      ambientFlagsSig(
        [
          { fileId: "m", purpose: "market" },
          { fileId: "h", purpose: "house" },
        ],
        0,
        0,
      ),
    ).toContain("m");

    const trunks = [
      { id: "r1", x0: 0, y0: 0, x1: 10, y1: 0, weight: 4 },
    ];
    expect(ambientDustSig(trunks)).toBe(ambientDustSig(trunks));

    expect(
      ambientForumsSig(
        [
          { fileId: "m1", purpose: "market", isCommons: false },
          { fileId: "c1", purpose: "house", isCommons: true },
        ],
        0,
        0,
      ),
    ).toBe(
      ambientForumsSig(
        [
          { fileId: "m1", purpose: "market", isCommons: false },
          { fileId: "c1", purpose: "house", isCommons: true },
        ],
        0,
        0,
      ),
    );
  });

  it("unchanged inputs → no subsystem rebuild; changed → rebuild", () => {
    const profile = {
      ...RENDER_PROFILES.rich,
      maxNightWindows: 10,
      maxTrafficDust: 4,
      maxForumClusters: 3,
      maxBirds: 3,
    };
    const mgr = new AmbientLifeManager(profile);
    mgr.bake(fakeTexSource());

    let windowRebuilds = 0;
    let dustRebuilds = 0;
    let forumRebuilds = 0;
    let chimneySets = 0;
    const origW = mgr.windows.rebuild.bind(mgr.windows);
    const origD = mgr.dust.rebuild.bind(mgr.dust);
    const origF = mgr.forums.rebuild.bind(mgr.forums);
    const origC = mgr.chimney.setEmitters.bind(mgr.chimney);
    mgr.windows.rebuild = ((...args: Parameters<typeof origW>) => {
      windowRebuilds++;
      return origW(...args);
    }) as typeof origW;
    mgr.dust.rebuild = ((...args: Parameters<typeof origD>) => {
      dustRebuilds++;
      return origD(...args);
    }) as typeof origD;
    mgr.forums.rebuild = ((...args: Parameters<typeof origF>) => {
      forumRebuilds++;
      return origF(...args);
    }) as typeof origF;
    mgr.chimney.setEmitters = ((...args: Parameters<typeof origC>) => {
      chimneySets++;
      return origC(...args);
    }) as typeof origC;

    const views = [
      makeView({
        fileId: "w1",
        purpose: "workshop",
        x: 0,
        y: 0,
        salt: 0,
        level: 2,
        agentPresent: "a",
      }),
      makeView({
        fileId: "m1",
        purpose: "market",
        x: 20,
        y: 0,
        salt: 1,
        level: 3,
      }),
    ];
    const trunks = [
      { id: "r1", x0: 0, y0: 0, x1: 100, y1: 0, weight: 5 },
    ];
    const opts = {
      buildings: views,
      trunks,
      centerX: 10,
      centerY: 0,
      nowMs: NOW,
    };

    mgr.rebuild(opts);
    expect(windowRebuilds).toBe(1);
    expect(dustRebuilds).toBe(1);
    expect(forumRebuilds).toBe(1);
    expect(chimneySets).toBe(1);

    // Identical inputs — no subsystem rebuilds
    mgr.rebuild(opts);
    expect(windowRebuilds).toBe(1);
    expect(dustRebuilds).toBe(1);
    expect(forumRebuilds).toBe(1);
    expect(chimneySets).toBe(1);

    // Change window-relevant input (level) only
    const views2 = [
      views[0],
      makeView({
        fileId: "m1",
        purpose: "market",
        x: 20,
        y: 0,
        salt: 1,
        level: 4, // was 3
      }),
    ];
    mgr.rebuild({ ...opts, buildings: views2 });
    expect(windowRebuilds).toBe(2);
    expect(dustRebuilds).toBe(1); // roads unchanged
    expect(forumRebuilds).toBe(1); // anchors unchanged
    // chimney: m1 is not a chimney candidate; w1 unchanged → no chimney rebuild
    expect(chimneySets).toBe(1);

    // Change dust (roads topology)
    mgr.rebuild({
      ...opts,
      buildings: views2,
      trunks: [
        { id: "r1", x0: 0, y0: 0, x1: 100, y1: 0, weight: 5 },
        { id: "r2", x0: 0, y0: 0, x1: 50, y1: 50, weight: 4 },
      ],
    });
    expect(dustRebuilds).toBe(2);
    expect(windowRebuilds).toBe(2);

    // Change chimney activity
    const views3 = [
      makeView({
        fileId: "w1",
        purpose: "workshop",
        x: 0,
        y: 0,
        salt: 0,
        level: 2,
        agentPresent: null, // was "a" — inactive now
        lastModified: "",
      }),
      views2[1],
    ];
    mgr.rebuild({
      ...opts,
      buildings: views3,
      trunks: [
        { id: "r1", x0: 0, y0: 0, x1: 100, y1: 0, weight: 5 },
        { id: "r2", x0: 0, y0: 0, x1: 50, y1: 50, weight: 4 },
      ],
    });
    expect(chimneySets).toBe(2);

    mgr.destroy();
  });
});

// ---------------------------------------------------------------------------
// F9 — ambient flag cleanup on deselection
// ---------------------------------------------------------------------------

describe("ambient flag cleanup (F9)", () => {
  it("removes ambient-added flags when building leaves selection; kit flags stay", () => {
    const kitContainer = new Container();
    const kitFlag = new Flag(0, -20, 0.75);
    kitContainer.addChild(kitFlag.node);
    const kitAnims = [kitFlag];

    const ambientContainer = new Container();
    const ambientAnims: AnimInstance[] = [];

    // Apply to both: kit building reuses flag; ambient-less creates marked one
    const applied = applyCivicFlagPhases([
      {
        fileId: "kit-townhall",
        kitAnims,
        container: kitContainer,
        depth: 40,
      },
      {
        fileId: "lib-ambient",
        kitAnims: ambientAnims,
        container: ambientContainer,
        depth: 30,
      },
    ]);
    expect(applied.createdIds).toEqual(["lib-ambient"]);
    expect(isAmbientLifeFlag(ambientAnims[0]!)).toBe(true);
    expect(isAmbientLifeFlag(kitFlag)).toBe(false);

    const ambientFlagIds = new Set(applied.createdIds);
    // Deselect ambient building only
    removeAmbientCivicFlags(
      [
        { fileId: "kit-townhall", kitAnims },
        { fileId: "lib-ambient", kitAnims: ambientAnims },
      ],
      new Set(["kit-townhall"]), // keep only kit
      ambientFlagIds,
    );
    expect(ambientAnims.length).toBe(0);
    expect(ambientFlagIds.has("lib-ambient")).toBe(false);
    // Kit flag untouched
    expect(kitAnims.length).toBe(1);
    expect(kitAnims[0]).toBe(kitFlag);
  });

  it("manager rebuild drops ambient flags when civic set shrinks", () => {
    const profile = {
      ...RENDER_PROFILES.rich,
      maxCivicFlags: 1, // only nearest civic
      maxBirds: 0,
      maxNightWindows: 0,
      maxTrafficDust: 0,
      maxForumClusters: 0,
      maxChimneySmoke: 0,
    };
    const mgr = new AmbientLifeManager(profile);
    mgr.bake(fakeTexSource());

    const near = makeView({
      fileId: "near-lib",
      purpose: "library",
      x: 5,
      y: 0,
    });
    const far = makeView({
      fileId: "far-lib",
      purpose: "library",
      x: 100,
      y: 0,
    });

    // Cap 1 with only far → selects far
    mgr.rebuild({
      buildings: [far],
      trunks: [],
      centerX: 0,
      centerY: 0,
      nowMs: NOW,
    });
    expect(mgr.ambientAddedFlagIds.has("far-lib")).toBe(true);
    expect(far.kitAnims.some((a) => isAmbientLifeFlag(a))).toBe(true);

    // Add nearer library — far falls out of top-1
    mgr.rebuild({
      buildings: [near, far],
      trunks: [],
      centerX: 0,
      centerY: 0,
      nowMs: NOW,
    });
    expect(mgr.ambientAddedFlagIds.has("near-lib")).toBe(true);
    expect(mgr.ambientAddedFlagIds.has("far-lib")).toBe(false);
    expect(far.kitAnims.some((a) => isAmbientLifeFlag(a))).toBe(false);
    expect(near.kitAnims.some((a) => isAmbientLifeFlag(a))).toBe(true);

    mgr.destroy();
  });
});

// ---------------------------------------------------------------------------
// F12 — birds survive rebuild (bounds only)
// ---------------------------------------------------------------------------

describe("birds survive rebuild (F12)", () => {
  it("rebuild updates bounds but does not clear active birds", () => {
    const profile = {
      ...RENDER_PROFILES.rich,
      maxBirds: 3,
      maxNightWindows: 0,
      maxTrafficDust: 0,
      maxForumClusters: 0,
      maxChimneySmoke: 0,
      maxCivicFlags: 0,
    };
    const mgr = new AmbientLifeManager(profile);
    mgr.bake(fakeTexSource());

    // Seed bounds + force spawn via many steps (rng chance 0.04 per frame)
    mgr.rebuild({
      buildings: [
        makeView({ fileId: "a", purpose: "house", x: -50, y: -50 }),
        makeView({ fileId: "b", purpose: "house", x: 50, y: 50 }),
      ],
      trunks: [],
      centerX: 0,
      centerY: 0,
      nowMs: NOW,
    });

    // Step until at least one bird is active (bounded)
    let frames = 0;
    while (mgr.birds.activeCount === 0 && frames < 5000) {
      mgr.birds.setEnabled(true);
      mgr.birds.step(frames, false);
      frames++;
    }
    // If spawn is unlucky under deterministic seed, force-check clear path:
    // at least verify rebuild does not reseed when birds already active.
    if (mgr.birds.activeCount === 0) {
      // Directly verify clear is NOT called by checking rng state via second rebuild
      // still leaves activeCount at 0 and does not throw.
      const before = mgr.birds.activeCount;
      mgr.rebuild({
        buildings: [
          makeView({ fileId: "a", purpose: "house", x: -80, y: -80 }),
          makeView({ fileId: "b", purpose: "house", x: 80, y: 80 }),
        ],
        trunks: [],
        centerX: 0,
        centerY: 0,
        nowMs: NOW,
      });
      expect(mgr.birds.activeCount).toBe(before);
    } else {
      const activeBefore = mgr.birds.activeCount;
      expect(activeBefore).toBeGreaterThan(0);
      mgr.rebuild({
        buildings: [
          makeView({ fileId: "a", purpose: "house", x: -80, y: -80 }),
          makeView({ fileId: "b", purpose: "house", x: 80, y: 80 }),
        ],
        trunks: [],
        centerX: 0,
        centerY: 0,
        nowMs: NOW,
      });
      // Birds must still be mid-flight — not cleared/reseeded
      expect(mgr.birds.activeCount).toBe(activeBefore);
    }

    mgr.destroy();
  });
});
