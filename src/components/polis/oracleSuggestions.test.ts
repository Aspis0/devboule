import { describe, it, expect } from "vitest";
import { buildOracleSuggestions, seedQuestions } from "./oracleSuggestions";
import type { CityState } from "../../types/city";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeCity(
  overrides: Partial<CityState> = {},
): CityState {
  return {
    version: 1,
    projectName: "test",
    era: "alpha",
    generatedAt: "2026-01-01",
    gridSize: { w: 10, h: 10 },
    districts: [],
    buildings: [],
    roads: [],
    agents: [],
    externalServices: [],
    notes: [],
    sins: [],
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("seedQuestions (generic, repo-agnostic)", () => {
  it("exposes starter questions that do not name this monorepo", () => {
    expect(seedQuestions.length).toBeGreaterThanOrEqual(3);
    const joined = seedQuestions.join(" ").toLowerCase();
    expect(joined).not.toMatch(/cloudflare|scaleway|scrna/);
    // Spot-check that they read as generic exploration prompts.
    expect(seedQuestions.some((q) => /project|architecture|test|entry/i.test(q))).toBe(
      true,
    );
  });
});

describe("buildOracleSuggestions", () => {
  it("returns seedQuestions fallback when city is null", () => {
    const result = buildOracleSuggestions(null);
    expect(result).toEqual([...seedQuestions]);
  });

  it("returns at least the seed questions when city has no buildings/districts/features", () => {
    const city = makeCity();
    const result = buildOracleSuggestions(city);
    // With no data the function pads from seedQuestions
    for (const q of seedQuestions) {
      expect(result).toContain(q);
    }
  });

  it("derives suggestions from feature labels", () => {
    const city = makeCity({
      features: [
        { id: "f1", label: "Worker Layer", description: "", colorAccent: "#aaa", kind: "domain" },
        { id: "f2", label: "Storage Module", description: "", colorAccent: "#bbb", kind: "domain" },
      ],
    });
    const result = buildOracleSuggestions(city);
    // Should contain questions derived from the labels
    expect(result.some((s) => s.includes("Storage Module"))).toBe(true);
    expect(result.some((s) => s.includes("Worker Layer"))).toBe(true);
  });

  it("falls back to district names when no features present", () => {
    const city = makeCity({
      districts: [
        {
          districtId: "d1",
          name: "Auth Area",
          type: "domain",
          bounds: { x: 0, y: 0, w: 5, h: 5 },
          wallStyle: "none",
          colorAccent: "#ccc",
        },
      ],
    });
    const result = buildOracleSuggestions(city);
    expect(result.some((s) => s.includes("Auth Area"))).toBe(true);
  });

  it("derives suggestions from prominent (heaviest) buildings", () => {
    const city = makeCity({
      buildings: [
        {
          fileId: "b1",
          filePath: "/src/api.ts",
          districtId: "d1",
          purpose: "library",
          purposeSource: "oracle",
          linesOfCode: 3000,
          visualTier: "mnemeion",
          coords: { x: 0, y: 0 },
          status: "normal",
          label: "ApiGateway",
          description: "",
          lastModified: "",
          sins: [],
          notes: [],
        },
        {
          fileId: "b2",
          filePath: "/src/small.ts",
          districtId: "d1",
          purpose: "house",
          purposeSource: "default",
          linesOfCode: 50,
          visualTier: "kalybe",
          coords: { x: 1, y: 0 },
          status: "normal",
          label: "SmallHelper",
          description: "",
          lastModified: "",
          sins: [],
          notes: [],
        },
      ],
    });
    const result = buildOracleSuggestions(city);
    // The heaviest building's label should appear in a suggestion
    expect(result.some((s) => s.includes("ApiGateway"))).toBe(true);
  });

  it("is deterministic — same input produces same output", () => {
    const city = makeCity({
      features: [
        { id: "f1", label: "Backend", description: "", colorAccent: "#aaa", kind: "domain" },
        { id: "f2", label: "Frontend", description: "", colorAccent: "#bbb", kind: "domain" },
      ],
      buildings: [
        {
          fileId: "b1",
          filePath: "/src/server.ts",
          districtId: "d1",
          purpose: "tower",
          purposeSource: "oracle",
          linesOfCode: 500,
          visualTier: "oikia",
          coords: { x: 0, y: 0 },
          status: "normal",
          label: "ServerCore",
          description: "",
          lastModified: "",
          sins: [],
          notes: [],
        },
      ],
    });
    const first = buildOracleSuggestions(city);
    const second = buildOracleSuggestions(city);
    expect(first).toEqual(second);
  });

  it("caps results at 6 suggestions", () => {
    const city = makeCity({
      features: Array.from({ length: 10 }, (_, i) => ({
        id: `f${i}`,
        label: `Feature ${i}`,
        description: "",
        colorAccent: "#aaa",
        kind: "domain" as const,
      })),
    });
    const result = buildOracleSuggestions(city);
    expect(result.length).toBeLessThanOrEqual(6);
  });

  it("skips the generic 'default' and 'commons' feature labels", () => {
    const city = makeCity({
      features: [
        { id: "f1", label: "default", description: "", colorAccent: "#aaa", kind: "default" },
        { id: "f2", label: "commons", description: "", colorAccent: "#bbb", kind: "commons" },
        { id: "f3", label: "RealDomain", description: "", colorAccent: "#ccc", kind: "domain" },
      ],
    });
    const result = buildOracleSuggestions(city);
    expect(result.some((s) => s.includes("default"))).toBe(false);
    expect(result.some((s) => s.includes("commons"))).toBe(false);
    expect(result.some((s) => s.includes("RealDomain"))).toBe(true);
  });
});
