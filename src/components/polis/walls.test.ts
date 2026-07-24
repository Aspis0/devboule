// District walls planner + road visual-style mapper tests.

import { describe, it, expect } from "vitest";
import type { Building, District, Road } from "../../types/city";
import {
  planDistrictWall,
  mapRoadVisualKind,
  waterTileSet,
  cityCenterFromBuildings,
  resolveWallStyle,
  countBuildingsInDistrict,
} from "./walls";

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

function mkDistrict(overrides: Partial<District> = {}): District {
  return {
    districtId: "d1",
    name: "Core",
    type: "feature",
    bounds: { x: 10, y: 10, w: 8, h: 8 },
    wallStyle: "roman_wall",
    colorAccent: "#aabbcc",
    ...overrides,
  };
}

function mkBuilding(
  fileId: string,
  districtId: string,
  x: number,
  y: number,
): Building {
  return {
    fileId,
    filePath: `${fileId}.ts`,
    districtId,
    purpose: "house",
    purposeSource: "default",
    linesOfCode: 10,
    visualTier: "kalybe",
    coords: { x, y },
    status: "normal",
    label: fileId,
    description: "",
    lastModified: "",
    sins: [],
    notes: [],
  };
}

/** N buildings inside a district (for size-threshold fixtures). */
function mkDistrictBuildings(
  n: number,
  districtId = "d1",
  baseX = 11,
  baseY = 11,
): Building[] {
  const out: Building[] = [];
  for (let i = 0; i < n; i++) {
    out.push(
      mkBuilding(
        `${districtId}-b${i}`,
        districtId,
        baseX + (i % 6),
        baseY + Math.floor(i / 6),
      ),
    );
  }
  return out;
}

function mkRoad(overrides: Partial<Road> & Pick<Road, "roadId" | "from" | "to">): Road {
  return {
    type: "import",
    style: "lastricata",
    weight: 1,
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// resolveWallStyle
// ---------------------------------------------------------------------------

describe("resolveWallStyle", () => {
  it("accepts the three drawable styles", () => {
    expect(resolveWallStyle("roman_wall")).toBe("roman_wall");
    expect(resolveWallStyle("aqueduct")).toBe("aqueduct");
    expect(resolveWallStyle("palisade")).toBe("palisade");
  });

  it("rejects none / unknown", () => {
    expect(resolveWallStyle("none")).toBeNull();
    expect(resolveWallStyle("")).toBeNull();
    expect(resolveWallStyle(undefined)).toBeNull();
    expect(resolveWallStyle("fancy")).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Size threshold (building count by districtId)
// ---------------------------------------------------------------------------

describe("planDistrictWall size threshold", () => {
  it("returns null for districts with < 6 buildings (3-building meadow)", () => {
    const buildings = mkDistrictBuildings(3);
    expect(countBuildingsInDistrict("d1", buildings)).toBe(3);
    expect(
      planDistrictWall(mkDistrict(), [], buildings, { cityCenter: { x: 0, y: 0 } }),
    ).toBeNull();
  });

  it("returns low variant for 6–9 buildings (8 → low kerb)", () => {
    const buildings = mkDistrictBuildings(8);
    const plan = planDistrictWall(mkDistrict(), [], buildings, {
      cityCenter: { x: 0, y: 0 },
    });
    expect(plan).not.toBeNull();
    expect(plan!.variant).toBe("low");
    expect(plan!.segments.length).toBeGreaterThan(0);
    expect(plan!.towers).toHaveLength(0);
    expect(plan!.gates).toHaveLength(0);
  });

  it("returns full variant for >= 10 buildings (12 → full wall)", () => {
    const buildings = mkDistrictBuildings(12);
    const plan = planDistrictWall(mkDistrict(), [], buildings, {
      cityCenter: { x: 0, y: 0 },
    });
    expect(plan).not.toBeNull();
    expect(plan!.variant).toBe("full");
    expect(plan!.segments.length).toBeGreaterThan(0);
    // 12 < 14 → full wall but no corner towers.
    expect(plan!.towers).toHaveLength(0);
    // Full walls always have at least the fallback gate.
    expect(plan!.gates.length).toBeGreaterThanOrEqual(1);
  });

  it("counts only buildings in this district (ignores foreign districtId)", () => {
    // 4 local + 10 foreign → still under threshold.
    const buildings = [
      ...mkDistrictBuildings(4, "d1"),
      ...mkDistrictBuildings(10, "other"),
    ];
    expect(countBuildingsInDistrict("d1", buildings)).toBe(4);
    expect(
      planDistrictWall(mkDistrict(), [], buildings, { cityCenter: { x: 0, y: 0 } }),
    ).toBeNull();
  });

  it("does not use bounds size — tiny bounds with 12 buildings still full", () => {
    const buildings = mkDistrictBuildings(12);
    const plan = planDistrictWall(
      mkDistrict({ bounds: { x: 10, y: 10, w: 2, h: 2 } }),
      [],
      buildings,
      { cityCenter: { x: 0, y: 0 } },
    );
    expect(plan).not.toBeNull();
    expect(plan!.variant).toBe("full");
  });
});

// ---------------------------------------------------------------------------
// planDistrictWall — determinism
// ---------------------------------------------------------------------------

describe("planDistrictWall determinism", () => {
  it("same input → identical plan", () => {
    const d = mkDistrict();
    // 12 in-district for a full wall + 1 foreign for the road endpoint.
    const buildings = [
      ...mkDistrictBuildings(12),
      mkBuilding("out", "d2", 30, 30),
    ];
    // Road from first in-district building to the foreign one.
    const roads = [
      mkRoad({
        roadId: "r1",
        from: "d1-b0",
        to: "out",
        path: [
          { x: 11, y: 11 },
          { x: 18, y: 14 },
          { x: 30, y: 30 },
        ],
      }),
    ];
    const opts = {
      waterTiles: waterTileSet([]),
      cityCenter: cityCenterFromBuildings(buildings),
    };
    const a = planDistrictWall(d, roads, buildings, opts);
    const b = planDistrictWall(d, roads, buildings, opts);
    expect(a).not.toBeNull();
    expect(a).toEqual(b);
  });

  it("returns null for wallStyle none", () => {
    expect(
      planDistrictWall(mkDistrict({ wallStyle: "none" }), [], mkDistrictBuildings(20)),
    ).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Gates
// ---------------------------------------------------------------------------

describe("planDistrictWall gates", () => {
  it("places a gate where an inter-district road crosses the boundary", () => {
    // District [10,10]..[18,18]. Road path exits east edge at x=18, y=14.
    // 12 in-district buildings for full wall.
    const d = mkDistrict();
    const buildings = [
      ...mkDistrictBuildings(12),
      mkBuilding("out", "other", 25, 14),
    ];
    const roads = [
      mkRoad({
        roadId: "cross",
        from: "d1-b0",
        to: "out",
        path: [
          { x: 12, y: 14 },
          { x: 18, y: 14 },
          { x: 25, y: 14 },
        ],
      }),
    ];
    const plan = planDistrictWall(d, roads, buildings, {
      cityCenter: { x: 0, y: 0 },
    });
    expect(plan).not.toBeNull();
    expect(plan!.variant).toBe("full");
    expect(plan!.gates.length).toBeGreaterThanOrEqual(1);
    // At least one gate on the east edge (side 1) near y=14.
    const east = plan!.gates.filter((g) => g.side === 1);
    expect(east.length).toBeGreaterThanOrEqual(1);
    expect(east.some((g) => Math.abs(g.y - 14) < 1)).toBe(true);
    // Segments must not cover the gate gap on that side.
    for (const seg of plan!.segments) {
      if (seg.side !== 1) continue;
      const coversGate = east.some(
        (g) =>
          g.y > Math.min(seg.ay, seg.by) + 0.01 &&
          g.y < Math.max(seg.ay, seg.by) - 0.01,
      );
      expect(coversGate).toBe(false);
    }
  });

  it("adds a fallback gate facing the city center when no road crosses", () => {
    const d = mkDistrict(); // centre ~ (14, 14)
    const buildings = mkDistrictBuildings(12);
    // City centre far to the east → fallback gate on east side (1).
    const plan = planDistrictWall(d, [], buildings, {
      cityCenter: { x: 100, y: 14 },
    });
    expect(plan).not.toBeNull();
    expect(plan!.gates).toHaveLength(1);
    expect(plan!.gates[0].side).toBe(1);
  });

  it("low variant has no gates even when roads cross", () => {
    const d = mkDistrict();
    const buildings = [
      ...mkDistrictBuildings(8),
      mkBuilding("out", "other", 25, 14),
    ];
    const roads = [
      mkRoad({
        roadId: "cross",
        from: "d1-b0",
        to: "out",
        path: [
          { x: 12, y: 14 },
          { x: 18, y: 14 },
          { x: 25, y: 14 },
        ],
      }),
    ];
    const plan = planDistrictWall(d, roads, buildings, {
      cityCenter: { x: 0, y: 0 },
    });
    expect(plan).not.toBeNull();
    expect(plan!.variant).toBe("low");
    expect(plan!.gates).toHaveLength(0);
  });
});

// ---------------------------------------------------------------------------
// Water skip
// ---------------------------------------------------------------------------

describe("planDistrictWall water skip", () => {
  it("skips wall segments whose tile is water", () => {
    const d = mkDistrict({
      bounds: { x: 0, y: 0, w: 6, h: 6 },
    });
    // Flood the entire north edge tiles with water.
    const water = waterTileSet([
      { gx: 0, gy: 0 },
      { gx: 1, gy: 0 },
      { gx: 2, gy: 0 },
      { gx: 3, gy: 0 },
      { gx: 4, gy: 0 },
      { gx: 5, gy: 0 },
    ]);
    const plan = planDistrictWall(d, [], mkDistrictBuildings(12), {
      waterTiles: water,
      cityCenter: { x: 3, y: 100 }, // gate on south so north is free
    });
    expect(plan).not.toBeNull();
    // No segment should sit entirely on the waterlogged north edge (side 0).
    // Sub-samples on water are dropped — north edge should have zero (or near-
    // zero) length segments.
    const northLen = plan!.segments
      .filter((s) => s.side === 0)
      .reduce((acc, s) => acc + Math.hypot(s.bx - s.ax, s.by - s.ay), 0);
    expect(northLen).toBeLessThan(0.5);
    // Other sides still have wall.
    const otherLen = plan!.segments
      .filter((s) => s.side !== 0)
      .reduce((acc, s) => acc + Math.hypot(s.bx - s.ax, s.by - s.ay), 0);
    expect(otherLen).toBeGreaterThan(1);
  });
});

// ---------------------------------------------------------------------------
// Roman towers (gated by building count >= 14)
// ---------------------------------------------------------------------------

describe("planDistrictWall roman towers", () => {
  it("roman_wall with >= 14 buildings has exactly 4 corner towers", () => {
    const plan = planDistrictWall(
      mkDistrict({ wallStyle: "roman_wall" }),
      [],
      mkDistrictBuildings(14),
      { cityCenter: { x: 0, y: 0 } },
    );
    expect(plan).not.toBeNull();
    expect(plan!.variant).toBe("full");
    expect(plan!.towers).toHaveLength(4);
    const corners = new Set(plan!.towers.map((t) => t.corner));
    expect(corners.size).toBe(4);
  });

  it("roman_wall with 10–13 buildings is full but has no towers", () => {
    const plan = planDistrictWall(
      mkDistrict({ wallStyle: "roman_wall" }),
      [],
      mkDistrictBuildings(12),
      { cityCenter: { x: 0, y: 0 } },
    );
    expect(plan).not.toBeNull();
    expect(plan!.variant).toBe("full");
    expect(plan!.towers).toHaveLength(0);
  });

  it("palisade and aqueduct have no corner towers even at 14+ buildings", () => {
    for (const style of ["palisade", "aqueduct"] as const) {
      const plan = planDistrictWall(
        mkDistrict({ wallStyle: style }),
        [],
        mkDistrictBuildings(16),
        { cityCenter: { x: 0, y: 0 } },
      );
      expect(plan).not.toBeNull();
      expect(plan!.variant).toBe("full");
      expect(plan!.towers).toHaveLength(0);
    }
  });

  it("low variant never has towers", () => {
    const plan = planDistrictWall(
      mkDistrict({ wallStyle: "roman_wall" }),
      [],
      mkDistrictBuildings(8),
      { cityCenter: { x: 0, y: 0 } },
    );
    expect(plan!.variant).toBe("low");
    expect(plan!.towers).toHaveLength(0);
  });
});

// ---------------------------------------------------------------------------
// mapRoadVisualKind
// ---------------------------------------------------------------------------

describe("mapRoadVisualKind", () => {
  it("maps clone roads (Rust wire: type clone + terra_battuta + ast) to dirt_dashed", () => {
    // Realistic wire object from scanner.rs clone emit (P4.2).
    expect(
      mapRoadVisualKind(
        { type: "clone", style: "terra_battuta", weight: 1, provenance: "ast" },
        false,
      ),
    ).toBe("dirt_dashed");
  });

  it("maps terra_battuta (non-semantic) to dirt_dashed", () => {
    expect(
      mapRoadVisualKind(
        { type: "import", style: "terra_battuta", weight: 1 },
        false,
      ),
    ).toBe("dirt_dashed");
  });

  it("maps semantic type/provenance to semantic_faint (even if terra_battuta)", () => {
    expect(
      mapRoadVisualKind(
        {
          type: "semantic",
          style: "terra_battuta",
          weight: 1,
          provenance: "semantic",
        },
        false,
      ),
    ).toBe("semantic_faint");
    expect(
      mapRoadVisualKind(
        { type: "import", style: "lastricata", weight: 1, provenance: "semantic" },
        true,
      ),
    ).toBe("semantic_faint");
  });

  it("maps trunk import/lastricata to cobble_edged", () => {
    expect(
      mapRoadVisualKind(
        { type: "import", style: "lastricata", weight: 5, provenance: "ast" },
        true,
      ),
    ).toBe("cobble_edged");
  });

  it("maps non-import trunk to plain cobble", () => {
    expect(
      mapRoadVisualKind(
        { type: "infrastructure", style: "acquedotto", weight: 4 },
        true,
      ),
    ).toBe("cobble");
  });

  it("maps ordinary minor import to minor", () => {
    expect(
      mapRoadVisualKind(
        { type: "import", style: "lastricata", weight: 1, provenance: "regex" },
        false,
      ),
    ).toBe("minor");
  });
});

// ---------------------------------------------------------------------------
// Edge cases: missing endpoints, degenerate bounds, water-aware fallback gate
// ---------------------------------------------------------------------------

describe("planDistrictWall missing-endpoint roads", () => {
  it("roads with an endpoint missing from buildings punch no gate", () => {
    const d = mkDistrict();
    // 12 in-district buildings for full wall; road to "ghost-out" is stale.
    const buildings = mkDistrictBuildings(12);
    const roads = [
      mkRoad({
        roadId: "stale",
        from: "d1-b0",
        to: "ghost-out",
        path: [
          { x: 12, y: 14 },
          { x: 18, y: 14 },
          { x: 25, y: 14 },
        ],
      }),
    ];
    const plan = planDistrictWall(d, roads, buildings, {
      cityCenter: { x: 100, y: 14 }, // east-facing fallback if no road gates
    });
    expect(plan).not.toBeNull();
    // No road-derived gate: only the single fallback gate remains.
    expect(plan!.gates).toHaveLength(1);
    expect(plan!.gates[0].side).toBe(1); // east fallback, not a path crossing
  });
});

describe("planDistrictWall degenerate bounds", () => {
  it("rejects zero / negative / non-finite bounds", () => {
    const buildings = mkDistrictBuildings(12);
    expect(
      planDistrictWall(
        mkDistrict({ bounds: { x: 0, y: 0, w: 0, h: 8 } }),
        [],
        buildings,
      ),
    ).toBeNull();
    expect(
      planDistrictWall(
        mkDistrict({ bounds: { x: 0, y: 0, w: 8, h: 0 } }),
        [],
        buildings,
      ),
    ).toBeNull();
    expect(
      planDistrictWall(
        mkDistrict({ bounds: { x: 0, y: 0, w: -2, h: 8 } }),
        [],
        buildings,
      ),
    ).toBeNull();
    expect(
      planDistrictWall(
        mkDistrict({ bounds: { x: 0, y: 0, w: 8, h: -1 } }),
        [],
        buildings,
      ),
    ).toBeNull();
    expect(
      planDistrictWall(
        mkDistrict({ bounds: { x: 0, y: 0, w: Number.NaN, h: 8 } }),
        [],
        buildings,
      ),
    ).toBeNull();
  });
});

describe("planDistrictWall fallback gate water probe", () => {
  it("avoids water when non-water probe points are available", () => {
    // District [0,0]..[10,10]. City centre east → fallback side = east (x=10).
    // Midpoint (10, 5) is water; t=0.35 → y≈3.5 is dry.
    const d = mkDistrict({
      bounds: { x: 0, y: 0, w: 10, h: 10 },
    });
    const water = waterTileSet([
      { gx: 10, gy: 5 }, // midpoint of east edge
      { gx: 9, gy: 5 },
    ]);
    const plan = planDistrictWall(d, [], mkDistrictBuildings(12), {
      waterTiles: water,
      cityCenter: { x: 100, y: 5 },
    });
    expect(plan).not.toBeNull();
    expect(plan!.gates).toHaveLength(1);
    const g = plan!.gates[0];
    expect(g.side).toBe(1);
    // Gate must not sit on the water tile at y≈5.
    expect(Math.floor(g.y)).not.toBe(5);
    // First non-water probe after 0.5 is 0.35 → y = 0 + 10*0.35 = 3.5
    expect(Math.abs(g.y - 3.5)).toBeLessThan(0.01);
  });
});
