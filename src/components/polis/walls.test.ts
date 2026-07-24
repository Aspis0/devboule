// District walls planner + road visual-style mapper tests.

import { describe, it, expect } from "vitest";
import type { Building, District, Road } from "../../types/city";
import {
  planDistrictWall,
  planBoundaryMarkers,
  mapRoadVisualKind,
  collectBuiltOutlines,
  isSegmentUrban,
  pointInBounds,
  drawUrbanHub,
  ROAD_GEOMETRY,
  waterTileSet,
  cityCenterFromBuildings,
  resolveWallStyle,
  countBuildingsInDistrict,
  builtOutlineBounds,
  builtFootprintArea,
  builtToEnclosedRatio,
  WALL_GEOMETRY,
  STELE_GEOMETRY,
  WALL_MIN_BUILT_RATIO,
  WALL_OUTLINE_MARGIN,
  buildingTileFootprint,
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

  it("does not use reserved district bounds size — tiny bounds with 12 buildings still full", () => {
    // CHANGED (step2): wall geometry follows built extents, not district.bounds.
    // Degenerate/tiny reserved bounds must not kill a dense building cluster.
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
  it("places a gate where an inter-district road crosses the wall outline", () => {
    // CHANGED (step2): gates sit on built outline, not reserved district.bounds.
    // Compact 4×3 of 1×1 houses at (10,10): footprint AABB [10,10]–[14,13],
    // outline with margin 1 → [9,9] w=6 h=5. East edge x=15, y∈[9,14].
    // Road exits east mid-edge at y=11.5.
    const d = mkDistrict({ bounds: { x: 0, y: 0, w: 50, h: 50 } });
    const buildings: Building[] = [];
    for (let i = 0; i < 12; i++) {
      buildings.push(
        mkBuilding("d1-b" + i, "d1", 10 + (i % 4), 10 + Math.floor(i / 4)),
      );
    }
    buildings.push(mkBuilding("out", "other", 30, 11.5));
    const outline = builtOutlineBounds("d1", buildings)!;
    const eastX = outline.x + outline.w;
    const midY = outline.y + outline.h / 2;
    const roads = [
      mkRoad({
        roadId: "cross",
        from: "d1-b0",
        to: "out",
        path: [
          { x: 12, y: midY },
          { x: eastX, y: midY },
          { x: 30, y: midY },
        ],
      }),
    ];
    const plan = planDistrictWall(d, roads, buildings, {
      cityCenter: { x: 0, y: 0 },
    });
    expect(plan).not.toBeNull();
    expect(plan!.variant).toBe("full");
    expect(plan!.gates.length).toBeGreaterThanOrEqual(1);
    const east = plan!.gates.filter((g) => g.side === 1);
    expect(east.length).toBeGreaterThanOrEqual(1);
    expect(east.some((g) => Math.abs(g.y - midY) < 1)).toBe(true);
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
    const d = mkDistrict();
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
    const outline = builtOutlineBounds("d1", buildings)!;
    const eastX = outline.x + outline.w;
    const midY = outline.y + outline.h / 2;
    const roads = [
      mkRoad({
        roadId: "cross",
        from: "d1-b0",
        to: "out",
        path: [
          { x: 12, y: midY },
          { x: eastX, y: midY },
          { x: 25, y: midY },
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
    // CHANGED (step2): water is tested on the built outline edge, not
    // district.bounds. Compact cluster at (2,2); flood the outline's north edge.
    const buildings = mkDistrictBuildings(12, "d1", 2, 2);
    const outline = builtOutlineBounds("d1", buildings)!;
    const waterTiles: { gx: number; gy: number }[] = [];
    for (let x = Math.floor(outline.x); x < Math.ceil(outline.x + outline.w); x++) {
      waterTiles.push({ gx: x, gy: Math.floor(outline.y) });
    }
    const d = mkDistrict({ bounds: { x: 0, y: 0, w: 40, h: 40 } });
    const plan = planDistrictWall(d, [], buildings, {
      waterTiles: waterTileSet(waterTiles),
      cityCenter: { x: outline.x + outline.w / 2, y: 100 }, // gate south
    });
    expect(plan).not.toBeNull();
    const northLen = plan!.segments
      .filter((s) => s.side === 0)
      .reduce((acc, s) => acc + Math.hypot(s.bx - s.ax, s.by - s.ay), 0);
    expect(northLen).toBeLessThan(0.5);
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

  it("maps ordinary urban minor import to urban_street", () => {
    expect(
      mapRoadVisualKind(
        { type: "import", style: "lastricata", weight: 1, provenance: "regex" },
        false,
        true,
      ),
    ).toBe("urban_street");
  });

  it("maps rural trunk import to country_track (not bright cobble lattice)", () => {
    expect(
      mapRoadVisualKind(
        { type: "import", style: "lastricata", weight: 5, provenance: "ast" },
        true,
        false,
      ),
    ).toBe("country_track");
  });

  it("maps rural non-trunk to country_track", () => {
    expect(
      mapRoadVisualKind(
        { type: "import", style: "lastricata", weight: 1 },
        false,
        false,
      ),
    ).toBe("country_track");
  });

  it("defaults isUrban=true so legacy two-arg trunk calls stay cobble", () => {
    expect(
      mapRoadVisualKind(
        { type: "import", style: "lastricata", weight: 5 },
        true,
      ),
    ).toBe("cobble_edged");
  });
});

// ---------------------------------------------------------------------------
// Urban / rural segment classification (STEP 4)
// ---------------------------------------------------------------------------

describe("isSegmentUrban / collectBuiltOutlines", () => {
  it("is deterministic for the same buildings", () => {
    const buildings = mkDistrictBuildings(8, "d1", 10, 10);
    const a = collectBuiltOutlines(buildings);
    const b = collectBuiltOutlines(buildings);
    expect(a).toEqual(b);
    expect(a.length).toBe(1);
  });

  it("classifies a segment inside the built outline as urban", () => {
    const buildings = mkDistrictBuildings(8, "d1", 10, 10);
    const outlines = collectBuiltOutlines(buildings);
    // buildings sit at (10..15, 10..11) roughly — center of outline is urban
    expect(isSegmentUrban({ x: 12, y: 11 }, { x: 13, y: 11 }, outlines)).toBe(
      true,
    );
  });

  it("classifies a segment far from any building as rural", () => {
    const buildings = mkDistrictBuildings(8, "d1", 10, 10);
    const outlines = collectBuiltOutlines(buildings);
    // Far away in empty meadow
    expect(isSegmentUrban({ x: 200, y: 200 }, { x: 210, y: 200 }, outlines)).toBe(
      false,
    );
  });

  it("collectBuiltOutlines matches builtOutlineBounds when pad=0", () => {
    const buildings = mkDistrictBuildings(6, "d1", 10, 10);
    const base = builtOutlineBounds("d1", buildings)!;
    const outlines = collectBuiltOutlines(buildings, 0);
    expect(outlines).toHaveLength(1);
    expect(outlines[0]).toEqual(base);
    // Inside outline → urban; well outside → rural
    const ix = base.x + base.w * 0.5;
    const iy = base.y + base.h * 0.5;
    expect(pointInBounds(ix, iy, outlines[0])).toBe(true);
    expect(isSegmentUrban({ x: ix, y: iy }, { x: ix + 1, y: iy }, outlines)).toBe(
      true,
    );
    const ox = base.x + base.w + 5;
    const oy = base.y + base.h + 5;
    expect(isSegmentUrban({ x: ox, y: oy }, { x: ox + 2, y: oy }, outlines)).toBe(
      false,
    );
  });

  it("explicit pad expands the urban zone", () => {
    const buildings = mkDistrictBuildings(6, "d1", 10, 10);
    const base = builtOutlineBounds("d1", buildings)!;
    const pad = 2;
    const outlines = collectBuiltOutlines(buildings, pad);
    const x = base.x + base.w + pad * 0.5;
    const y = base.y + base.h * 0.5;
    expect(pointInBounds(x, y, outlines[0])).toBe(true);
    expect(isSegmentUrban({ x, y }, { x: x + 1, y }, outlines)).toBe(true);
  });

  it("returns rural when there are no buildings / empty outlines", () => {
    expect(isSegmentUrban({ x: 0, y: 0 }, { x: 1, y: 0 }, [])).toBe(false);
    expect(collectBuiltOutlines([])).toEqual([]);
  });
});

// ---------------------------------------------------------------------------
// Urban hub / junction handling (STEP 4)
// ---------------------------------------------------------------------------

describe("drawUrbanHub junction floor", () => {
  it("draws nothing for n < 2 (single kink not multi-route)", () => {
    // Graphics is pixi — we only assert the pure early-return count.
    const g = { circle: () => ({ fill: () => {} }) } as unknown as import("pixi.js").Graphics;
    expect(drawUrbanHub(g, 0, 0, 0)).toBe(0);
    expect(drawUrbanHub(g, 0, 0, 1)).toBe(0);
  });

  it("draws a disc for corners (n=2), T-junctions (n=3), and hubs (n≥4)", () => {
    const g = { circle: () => ({ fill: () => {} }) } as unknown as import("pixi.js").Graphics;
    expect(drawUrbanHub(g, 0, 0, 2)).toBe(1);
    expect(drawUrbanHub(g, 0, 0, 3)).toBe(1);
    expect(drawUrbanHub(g, 0, 0, 4)).toBe(1);
    expect(drawUrbanHub(g, 0, 0, 8)).toBe(1);
  });

  it("exposes geometry constants used by the renderer", () => {
    expect(ROAD_GEOMETRY.urbanStreetWidth).toBeGreaterThan(
      ROAD_GEOMETRY.countryTrackWidth,
    );
    expect(ROAD_GEOMETRY.urbanCapRadius).toBeGreaterThan(0);
    expect(ROAD_GEOMETRY.urbanHubRadius).toBeGreaterThanOrEqual(
      ROAD_GEOMETRY.urbanCapRadius,
    );
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
  it("still plans when reserved district.bounds are degenerate (uses built extents)", () => {
    // CHANGED (step2): district.bounds no longer gate presence. A dense cluster
    // with zero/negative reserved bounds still gets a wall from built extents.
    // Old assertion (null on bad district.bounds) is intentionally replaced.
    const buildings = mkDistrictBuildings(12);
    for (const bounds of [
      { x: 0, y: 0, w: 0, h: 8 },
      { x: 0, y: 0, w: 8, h: 0 },
      { x: 0, y: 0, w: -2, h: 8 },
      { x: 0, y: 0, w: 8, h: -1 },
      { x: 0, y: 0, w: Number.NaN, h: 8 },
    ]) {
      const plan = planDistrictWall(mkDistrict({ bounds }), [], buildings, {
        cityCenter: { x: 0, y: 0 },
      });
      expect(plan).not.toBeNull();
      expect(plan!.variant).toBe("full");
    }
  });

  it("returns null when there are no member buildings (no outline)", () => {
    expect(
      planDistrictWall(mkDistrict(), [], [], { cityCenter: { x: 0, y: 0 } }),
    ).toBeNull();
  });
});

describe("planDistrictWall fallback gate water probe", () => {
  it("avoids water when non-water probe points are available", () => {
    // CHANGED (step2): probe runs on built outline east edge, not district.bounds.
    const buildings = mkDistrictBuildings(12, "d1", 0, 0);
    const outline = builtOutlineBounds("d1", buildings)!;
    const eastX = outline.x + outline.w;
    const midY = outline.y + outline.h * 0.5;
    const probeY = outline.y + outline.h * 0.35;
    const d = mkDistrict({ bounds: { x: -50, y: -50, w: 100, h: 100 } });
    const water = waterTileSet([
      { gx: Math.floor(eastX), gy: Math.floor(midY) },
      { gx: Math.floor(eastX) - 1, gy: Math.floor(midY) },
    ]);
    const plan = planDistrictWall(d, [], buildings, {
      waterTiles: water,
      cityCenter: { x: 100, y: midY },
    });
    expect(plan).not.toBeNull();
    expect(plan!.gates).toHaveLength(1);
    const g = plan!.gates[0];
    expect(g.side).toBe(1);
    // Gate must not sit on the water tile at midY.
    expect(Math.floor(g.y)).not.toBe(Math.floor(midY));
    // First non-water probe after 0.5 is 0.35.
    expect(Math.abs(g.y - probeY)).toBeLessThan(0.01);
  });
});

// ---------------------------------------------------------------------------
// Step 2 regressions: built outline, emptiness, wall mass
// ---------------------------------------------------------------------------

describe("planDistrictWall built outline (not reserved bounds)", () => {
  it("segments hug member building extents, not the reserved district box", () => {
    // Huge reserved box; buildings only in a tight cluster around (20,20).
    const d = mkDistrict({ bounds: { x: 0, y: 0, w: 80, h: 80 } });
    const buildings: Building[] = [];
    for (let i = 0; i < 12; i++) {
      buildings.push(
        mkBuilding("d1-b" + i, "d1", 20 + (i % 4), 20 + Math.floor(i / 4)),
      );
    }
    const outline = builtOutlineBounds("d1", buildings)!;
    const plan = planDistrictWall(d, [], buildings, {
      cityCenter: { x: 0, y: 0 },
    });
    expect(plan).not.toBeNull();
    // Every segment endpoint must lie on the built outline (tolerance for
    // float), never out on the reserved 80×80 rim.
    for (const seg of plan!.segments) {
      for (const [x, y] of [
        [seg.ax, seg.ay],
        [seg.bx, seg.by],
      ] as const) {
        const onOutline =
          Math.abs(x - outline.x) < 1e-6 ||
          Math.abs(x - (outline.x + outline.w)) < 1e-6 ||
          Math.abs(y - outline.y) < 1e-6 ||
          Math.abs(y - (outline.y + outline.h)) < 1e-6;
        expect(onOutline).toBe(true);
        // Far from reserved outer rim (0 or 80).
        expect(x).toBeGreaterThan(10);
        expect(x).toBeLessThan(40);
        expect(y).toBeGreaterThan(10);
        expect(y).toBeLessThan(40);
      }
    }
    // Outline is far smaller than reserved bounds.
    expect(outline.w * outline.h).toBeLessThan(d.bounds.w * d.bounds.h * 0.1);
  });

  it("outline expands by WALL_OUTLINE_MARGIN beyond footprints", () => {
    const buildings = [
      mkBuilding("a", "d1", 10, 10),
      mkBuilding("b", "d1", 11, 10),
      mkBuilding("c", "d1", 10, 11),
      mkBuilding("d", "d1", 11, 11),
      mkBuilding("e", "d1", 12, 10),
      mkBuilding("f", "d1", 12, 11),
      mkBuilding("g", "d1", 10, 12),
      mkBuilding("h", "d1", 11, 12),
      mkBuilding("i", "d1", 12, 12),
      mkBuilding("j", "d1", 13, 10),
      mkBuilding("k", "d1", 13, 11),
      mkBuilding("l", "d1", 13, 12),
    ];
    // house/kalybe = 1×1 → raw AABB [10,10]–[14,13]
    const outline = builtOutlineBounds("d1", buildings)!;
    expect(outline.x).toBe(10 - WALL_OUTLINE_MARGIN);
    expect(outline.y).toBe(10 - WALL_OUTLINE_MARGIN);
    expect(outline.w).toBe(4 + 2 * WALL_OUTLINE_MARGIN);
    expect(outline.h).toBe(3 + 2 * WALL_OUTLINE_MARGIN);
  });
});

describe("planDistrictWall emptiness guard", () => {
  it("returns null when buildings are sparse relative to their outline", () => {
    // 10 buildings at the corners of a huge empty field → density << threshold.
    const d = mkDistrict({ bounds: { x: 0, y: 0, w: 100, h: 100 } });
    const spots = [
      [0, 0],
      [0, 1],
      [1, 0],
      [50, 0],
      [50, 1],
      [0, 50],
      [1, 50],
      [50, 50],
      [49, 50],
      [50, 49],
    ];
    const buildings = spots.map(([x, y], i) =>
      mkBuilding("s" + i, "d1", x, y),
    );
    const ratio = builtToEnclosedRatio("d1", buildings);
    expect(ratio).toBeLessThan(WALL_MIN_BUILT_RATIO);
    expect(
      planDistrictWall(d, [], buildings, { cityCenter: { x: 25, y: 25 } }),
    ).toBeNull();
  });

  it("still walls a compact cluster above the density floor", () => {
    const buildings = mkDistrictBuildings(12);
    const ratio = builtToEnclosedRatio("d1", buildings);
    expect(ratio).toBeGreaterThanOrEqual(WALL_MIN_BUILT_RATIO);
    expect(
      planDistrictWall(mkDistrict(), [], buildings, {
        cityCenter: { x: 0, y: 0 },
      }),
    ).not.toBeNull();
  });

  it("natural layout density (~0.10–0.20) gets no wall; dense pack does", () => {
    // Step 2b: walls are rare. GAP=3-ish packing sits ~0.10–0.20 on the real
    // fixture — must NOT wall. Tight 1-tile packing stays above 0.25 and walls.
    const natural: Building[] = [];
    // 12 × 1×1 houses on a 4×3 grid with origin step 3 (GAP-like spacing).
    // Span: x 0..10, y 0..7 → outline (margin 1) 12×9 = 108, ratio = 12/108 ≈ 0.111.
    for (let i = 0; i < 12; i++) {
      natural.push(
        mkBuilding(`n${i}`, "d1", (i % 4) * 3, Math.floor(i / 4) * 3),
      );
    }
    const naturalRatio = builtToEnclosedRatio("d1", natural);
    expect(naturalRatio).toBeGreaterThanOrEqual(0.1);
    expect(naturalRatio).toBeLessThan(0.2);
    expect(naturalRatio).toBeLessThan(WALL_MIN_BUILT_RATIO);
    expect(
      planDistrictWall(mkDistrict(), [], natural, {
        cityCenter: { x: 5, y: 5 },
      }),
    ).toBeNull();

    const dense = mkDistrictBuildings(12);
    const denseRatio = builtToEnclosedRatio("d1", dense);
    expect(denseRatio).toBeGreaterThanOrEqual(WALL_MIN_BUILT_RATIO);
    expect(
      planDistrictWall(mkDistrict(), [], dense, {
        cityCenter: { x: 0, y: 0 },
      }),
    ).not.toBeNull();
  });

  it("pins WALL_MIN_BUILT_RATIO at the step-2b density gate", () => {
    // Raised from 0.1 so sparse GAP=3 fabric stays unwalled.
    expect(WALL_MIN_BUILT_RATIO).toBe(0.25);
  });
});

describe("boundary stelae (soft place markers)", () => {
  it("plans four markers at built-outline corners when count ≥ 6", () => {
    const buildings = mkDistrictBuildings(8);
    const outline = builtOutlineBounds("d1", buildings)!;
    const markers = planBoundaryMarkers("d1", buildings);
    expect(markers).not.toBeNull();
    expect(markers!).toHaveLength(4);
    // NW, NE, SE, SW of the built outline.
    expect(markers![0]).toMatchObject({ x: outline.x, y: outline.y, corner: 0 });
    expect(markers![1]).toMatchObject({
      x: outline.x + outline.w,
      y: outline.y,
      corner: 1,
    });
    expect(markers![2]).toMatchObject({
      x: outline.x + outline.w,
      y: outline.y + outline.h,
      corner: 2,
    });
    expect(markers![3]).toMatchObject({
      x: outline.x,
      y: outline.y + outline.h,
      corner: 3,
    });
  });

  it("returns null below the low-wall count floor", () => {
    const buildings = mkDistrictBuildings(5);
    expect(planBoundaryMarkers("d1", buildings)).toBeNull();
  });

  it("stele mass stays readable at working zoom (not ticks)", () => {
    expect(STELE_GEOMETRY.w).toBeGreaterThanOrEqual(4);
    expect(STELE_GEOMETRY.h).toBeGreaterThanOrEqual(7);
    // At scale 0.85: ~7.6px tall block; at 0.3 LOD floor: ~2.7px still a mark.
    expect(STELE_GEOMETRY.h * 0.85).toBeGreaterThan(6);
    expect(STELE_GEOMETRY.h * 0.3).toBeGreaterThan(2);
  });
});

describe("wall mass geometry", () => {
  it("full wall height and thickness stay above hairline constants", () => {
    // Regression: a 4px wall reads as a fence. Keep mass.
    expect(WALL_GEOMETRY.wallH).toBeGreaterThanOrEqual(12);
    expect(WALL_GEOMETRY.bandW).toBeGreaterThanOrEqual(6);
    expect(WALL_GEOMETRY.lowWallH).toBeGreaterThanOrEqual(5);
    expect(WALL_GEOMETRY.lowBandW).toBeGreaterThanOrEqual(3);
  });

  it("on-screen size at viewport 0.85 and 0.3 matches constants × scale", () => {
    // Pure arithmetic from constants — no live render required.
    const scales = [0.85, 0.3] as const;
    for (const s of scales) {
      expect(WALL_GEOMETRY.wallH * s).toBeCloseTo(14 * s, 5);
      expect(WALL_GEOMETRY.bandW * s).toBeCloseTo(7 * s, 5);
    }
    // At LOD floor (0.3) full wall is still > 3px tall (was 1.2px at WALL_H=4).
    expect(WALL_GEOMETRY.wallH * 0.3).toBeGreaterThan(3);
    // At working zoom (0.85) full wall is a solid ~12px band.
    expect(WALL_GEOMETRY.wallH * 0.85).toBeGreaterThan(10);
  });

  it("buildingTileFootprint mirrors kit tiers for house/temple", () => {
    expect(buildingTileFootprint("house", "kalybe")).toEqual({ w: 1, d: 1 });
    expect(buildingTileFootprint("house", "synoikia")).toEqual({ w: 2, d: 2 });
    expect(buildingTileFootprint("temple", "mnemeion")).toEqual({ w: 4, d: 6 });
    expect(buildingTileFootprint("unknown-purpose", "kalybe")).toEqual({
      w: 1,
      d: 1,
    });
  });

  it("builtFootprintArea sums tile footprints", () => {
    const buildings = [
      mkBuilding("a", "d1", 0, 0), // house kalybe 1×1
      mkBuilding("b", "d1", 2, 0),
    ];
    buildings[0].purpose = "temple";
    buildings[0].visualTier = "kalybe"; // 2×3 = 6
    expect(builtFootprintArea("d1", buildings)).toBe(6 + 1);
  });
});
