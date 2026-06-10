import { describe, it, expect } from "vitest";
import { findBuildingByCitation } from "./findBuildingByCitation";
import type { Building, CityState } from "../../types/city";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeBuilding(filePath: string, fileId = filePath): Building {
  return {
    fileId,
    filePath,
    districtId: "d1",
    purpose: "library",
    purposeSource: "oracle",
    linesOfCode: 100,
    visualTier: "oikia",
    coords: { x: 0, y: 0 },
    status: "normal",
    label: fileId,
    description: "",
    lastModified: "",
    sins: [],
    notes: [],
  };
}

function makeCity(buildings: Building[]): CityState {
  return {
    version: 1,
    projectName: "test",
    era: "alpha",
    generatedAt: "2026-01-01",
    gridSize: { w: 10, h: 10 },
    districts: [],
    buildings,
    roads: [],
    agents: [],
    externalServices: [],
    notes: [],
    sins: [],
  };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("findBuildingByCitation", () => {
  it("returns null for null city", () => {
    expect(findBuildingByCitation(null, "src/worker.ts")).toBeNull();
  });

  it("returns null for empty fileSource", () => {
    const city = makeCity([makeBuilding("/project/src/worker.ts")]);
    expect(findBuildingByCitation(city, "")).toBeNull();
    expect(findBuildingByCitation(city, "  ")).toBeNull();
  });

  it("returns null when no building matches", () => {
    const city = makeCity([makeBuilding("/project/src/worker.ts")]);
    expect(findBuildingByCitation(city, "src/other.ts")).toBeNull();
  });

  it("exact match: building filePath === fileSource", () => {
    const b = makeBuilding("src/worker.ts");
    const city = makeCity([b]);
    expect(findBuildingByCitation(city, "src/worker.ts")).toBe(b);
  });

  it("suffix match: index-relative path matches absolute building path", () => {
    const b = makeBuilding("/home/user/project/src/worker.ts");
    const city = makeCity([b]);
    // Oracle citation is index-root-relative
    expect(findBuildingByCitation(city, "src/worker.ts")).toBe(b);
  });

  it("suffix match: deeper relative path", () => {
    const b = makeBuilding("/abs/path/to/project/components/polis/chunk.ts");
    const city = makeCity([b]);
    expect(findBuildingByCitation(city, "components/polis/chunk.ts")).toBe(b);
  });

  it("separator normalization: backslash in fileSource matches forward-slash in filePath", () => {
    const b = makeBuilding("/project/src/worker.ts");
    const city = makeCity([b]);
    // Windows-style citation path
    expect(findBuildingByCitation(city, "src\\worker.ts")).toBe(b);
  });

  it("separator normalization: backslash in filePath matches forward-slash citation", () => {
    const b = makeBuilding("C:\\project\\src\\worker.ts");
    const city = makeCity([b]);
    expect(findBuildingByCitation(city, "src/worker.ts")).toBe(b);
  });

  it("does NOT match a partial filename that is not at a path boundary", () => {
    // "worker.ts" should NOT match "other_worker.ts" via suffix
    const b = makeBuilding("/project/other_worker.ts");
    const city = makeCity([b]);
    // "worker.ts" is a suffix of "other_worker.ts" but there's no "/" before it
    expect(findBuildingByCitation(city, "worker.ts")).toBeNull();
  });

  it("returns the first matching building when multiple buildings are present", () => {
    const b1 = makeBuilding("/project/src/worker.ts", "b1");
    const b2 = makeBuilding("/project/test/worker.ts", "b2");
    const city = makeCity([b1, b2]);
    // Both have "src/worker.ts" suffix? No — b2 has "test/worker.ts".
    // "src/worker.ts" uniquely matches b1.
    expect(findBuildingByCitation(city, "src/worker.ts")).toBe(b1);
  });
});
