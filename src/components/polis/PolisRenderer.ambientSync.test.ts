// applyCityDiffInner ambient-life gate: sin/status/suspect-only churn must NOT
// invoke syncAmbientLife; agentPresent (and other ambient-relevant fields) must.
//
// Headless harness: Object.create the renderer, wire a single building node, stub
// heavy collaborators, spy on the private syncAmbientLife method.

import { describe, it, expect, vi } from "vitest";

const { PolisRenderer } = await import("./PolisRenderer");
import type { Building, CityState, SinSeverity, UrbanSin } from "../../types/city";

type AnyRec = Record<string, unknown>;

/** Access private static signature helpers used to seed last*Sig fields. */
const Static = PolisRenderer as unknown as {
  roadSignature: (roads: readonly unknown[]) => string;
  terrainSignature: (t?: unknown) => string;
  districtsHash: (d: readonly unknown[]) => string;
};

function mkBuilding(over: Partial<Building> = {}): Building {
  return {
    fileId: "f1",
    filePath: "src/a.ts",
    districtId: "d1",
    purpose: "house",
    purposeSource: "grounded",
    linesOfCode: 100,
    visualTier: "synoikia",
    coords: { x: 0, y: 0 },
    status: "idle",
    label: "a.ts",
    description: "",
    lastModified: "2026-01-01T00:00:00.000Z",
    agentPresent: undefined,
    suspectOfCardId: undefined,
    sins: [],
    notes: [],
    ...over,
  } as Building;
}

function mkCity(buildings: Building[]): CityState {
  return {
    buildings,
    roads: [],
    districts: [],
    agents: [],
    gridSize: { w: 16, h: 16 },
    externalServices: [],
  } as unknown as CityState;
}

function sin(severity: SinSeverity = "fire"): UrbanSin {
  return {
    sinId: "s1",
    severity,
    description: "test",
    autoDetectable: true,
  };
}

function makeHarness(building: Building) {
  const fake = Object.create(
    PolisRenderer.prototype,
  ) as InstanceType<typeof PolisRenderer>;
  const set = (k: string, v: unknown) => {
    (fake as unknown as AnyRec)[k] = v;
  };

  const node = {
    building,
    iso: { x: 0, y: 0 },
    container: {},
    labelDepth: 0,
    hitRadius: 8,
    kitAnims: [],
  };

  set("destroyed", false);
  set("buildingNodes", new Map([[building.fileId, node]]));
  set("selectedId", null);
  set("cullDirty", false);
  set("lastCity", null);
  set("filterSets", null);
  // Seed signatures so graphRebuilt / districtsChanged stay false.
  set("lastRoadSig", Static.roadSignature.call(PolisRenderer, []));
  set("lastTerrainSig", Static.terrainSignature.call(PolisRenderer, undefined));
  set("lastDistrictsHash", Static.districtsHash.call(PolisRenderer, []));

  set("growthFx", {
    popIn: vi.fn(),
    dust: vi.fn(),
    growTransition: vi.fn(),
    seal: vi.fn(),
    rubble: vi.fn(),
    cancelTransition: vi.fn(),
  });

  const syncAmbientLife = vi.fn();
  set("syncAmbientLife", syncAmbientLife);
  set("syncAmbient", vi.fn());
  set("reconcileAgents", vi.fn());
  set("syncTradeRoutes", vi.fn());
  set("syncRoadHitLayer", vi.fn());
  set("redrawRoads", vi.fn());
  set("redrawDistricts", vi.fn());
  set("redrawTerrainProps", vi.fn());
  set("drawSelectionRing", vi.fn());
  set("applyFilter", vi.fn());
  set("createBuildingNode", vi.fn());
  set("destroyBuildingNode", vi.fn());
  set("updateBuildingNodeInPlace", function (
    _n: { building: Building },
    b: Building,
  ) {
    _n.building = b;
    return _n;
  });
  set("externalLayer", { setServices: vi.fn() });
  set("agentLayer", { setBlocked: vi.fn() });
  set("ambientLayer", { setBlocked: vi.fn() });

  return { fake, syncAmbientLife };
}

function applyDiff(
  fake: InstanceType<typeof PolisRenderer>,
  next: CityState,
): void {
  (
    fake as unknown as { applyCityDiffInner: (n: CityState) => void }
  ).applyCityDiffInner(next);
}

describe("applyCityDiffInner — ambient life gate", () => {
  it("does not call syncAmbientLife for a sins-only building diff", () => {
    const base = mkBuilding();
    const { fake, syncAmbientLife } = makeHarness(base);
    applyDiff(fake, mkCity([mkBuilding({ sins: [sin("fire")] })]));
    expect(syncAmbientLife).not.toHaveBeenCalled();
  });

  it("calls syncAmbientLife when agentPresent changes", () => {
    const base = mkBuilding();
    const { fake, syncAmbientLife } = makeHarness(base);
    applyDiff(fake, mkCity([mkBuilding({ agentPresent: "agent-1" })]));
    expect(syncAmbientLife).toHaveBeenCalledTimes(1);
  });

  it("does not call syncAmbientLife for status-only or suspect-only churn", () => {
    const { fake: fakeStatus, syncAmbientLife: spyStatus } = makeHarness(
      mkBuilding(),
    );
    applyDiff(fakeStatus, mkCity([mkBuilding({ status: "active" })]));
    expect(spyStatus).not.toHaveBeenCalled();

    const { fake: fakeSuspect, syncAmbientLife: spySuspect } = makeHarness(
      mkBuilding(),
    );
    applyDiff(
      fakeSuspect,
      mkCity([mkBuilding({ suspectOfCardId: "card-1" })]),
    );
    expect(spySuspect).not.toHaveBeenCalled();
  });
});
