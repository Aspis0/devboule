// @vitest-environment jsdom
// citySignature.test.ts — unit tests for the citySignature hash/walk function.
//
// Pins the critical "skip-if-unchanged" correctness: two cities that differ ONLY
// in the excluded volatile fields (generatedAt, per-building lastModified) MUST
// produce the same `sig`; any other structural change MUST produce a different
// sig. Also verifies `chars` is a monotonic-ish size proxy.

import { describe, expect, it } from "vitest";
import type { CityState } from "../types/city";
import { citySignature } from "./cityStore";

/** Minimal but shape-correct CityState for a signature test. */
function makeCity(overrides: Partial<CityState> = {}): CityState {
  return {
    version: 1,
    projectName: "test",
    era: "Alpha",
    generatedAt: "2025-01-01T00:00:00Z",
    gridSize: { w: 100, h: 100 },
    districts: [],
    buildings: [
      {
        fileId: "b1",
        filePath: "src/a.ts",
        districtId: "core",
        purpose: "house",
        purposeSource: "default",
        featureId: "commons",
        featureSource: "commons",
        linesOfCode: 120,
        visualTier: "kalybe",
        coords: { x: 0, y: 0 },
        status: "normal",
        label: "a.ts",
        description: "",
        lastModified: "2025-01-01T00:00:00Z",
        agentPresent: undefined,
        kanbanCardId: undefined,
        untrackedChange: undefined,
        sins: [],
        notes: [],
      },
    ],
    roads: [],
    agents: [],
    externalServices: [],
    features: [],
    notes: [],
    sins: [],
    ...overrides,
  } as CityState;
}

describe("citySignature", () => {
  it("identical except generatedAt + lastModified => same sig", () => {
    const a = makeCity();
    const b = makeCity({
      generatedAt: "2026-07-16T12:00:00Z",
      buildings: [
        {
          ...makeCity().buildings[0],
          lastModified: "2026-07-16T12:00:00Z",
        },
      ],
    });
    const sa = citySignature(a);
    const sb = citySignature(b);
    expect(sa.sig).toBe(sb.sig);
  });

  it("a city with one extra building => different sig", () => {
    const a = makeCity();
    const b = makeCity({
      buildings: [
        ...makeCity().buildings,
        {
          fileId: "b2",
          filePath: "src/b.ts",
          districtId: "core",
          purpose: "tower",
          purposeSource: "default",
          featureId: "commons",
          featureSource: "commons",
          linesOfCode: 80,
          visualTier: "kalybe",
          coords: { x: 1, y: 0 },
          status: "normal",
          label: "b.ts",
          description: "",
          lastModified: "",
          agentPresent: undefined,
          kanbanCardId: undefined,
          untrackedChange: undefined,
          sins: [],
          notes: [],
        },
      ],
    });
    expect(citySignature(a).sig).not.toBe(citySignature(b).sig);
  });

  it("building field changed (linesOfCode) => different sig", () => {
    const a = makeCity();
    const b = makeCity({
      buildings: [
        { ...makeCity().buildings[0], linesOfCode: 9999 },
      ],
    });
    expect(citySignature(a).sig).not.toBe(citySignature(b).sig);
  });

  it("a road added => different sig", () => {
    const a = makeCity();
    const b = makeCity({
      roads: [
        {
          roadId: "r1",
          from: "b1",
          to: "b1",
          type: "import" as const,
          style: "terra_battuta" as const,
          weight: 1,
        },
      ],
    });
    expect(citySignature(a).sig).not.toBe(citySignature(b).sig);
  });

  it("agent added => different sig", () => {
    const a = makeCity();
    const b = makeCity({
      agents: [
        {
          agentId: "orch-1",
          type: "orchestrator",
          status: "idle",
          currentFileId: null,
          currentTask: null,
          color: "#ff0000",
        },
      ],
    });
    expect(citySignature(a).sig).not.toBe(citySignature(b).sig);
  });

  it("agent removed (present vs empty) => different sig", () => {
    const withAgent = makeCity({
      agents: [
        {
          agentId: "orch-1",
          type: "orchestrator",
          status: "idle",
          currentFileId: null,
          currentTask: null,
          color: "#ff0000",
        },
      ],
    });
    const withoutAgent = makeCity(); // agents: []
    expect(citySignature(withAgent).sig).not.toBe(
      citySignature(withoutAgent).sig,
    );
  });

  it("chars > 0 for a non-trivial city", () => {
    const c = makeCity({
      buildings: [
        makeCity().buildings[0],
        {
          fileId: "b2",
          filePath: "src/b.ts",
          districtId: "core",
          purpose: "tower",
          purposeSource: "default",
          featureId: "commons",
          featureSource: "commons",
          linesOfCode: 200,
          visualTier: "oikia",
          coords: { x: 1, y: 0 },
          status: "normal",
          label: "b.ts",
          description: "",
          lastModified: "",
          agentPresent: undefined,
          kanbanCardId: undefined,
          untrackedChange: undefined,
          sins: [],
          notes: [],
        },
      ],
      agents: [
        {
          agentId: "orch-1",
          type: "orchestrator",
          status: "idle",
          currentFileId: null,
          currentTask: null,
          color: "#ff0000",
        },
      ],
    });
    const { chars } = citySignature(c);
    expect(chars).toBeGreaterThan(0);
  });

  it("chars grows when the city grows (monotonic-ish size proxy)", () => {
    const small = makeCity(); // 1 building, 0 agents, 0 roads
    const large = makeCity({
      buildings: [
        makeCity().buildings[0],
        {
          fileId: "b2",
          filePath: "src/b.ts",
          districtId: "core",
          purpose: "tower",
          purposeSource: "default",
          featureId: "commons",
          featureSource: "commons",
          linesOfCode: 200,
          visualTier: "oikia",
          coords: { x: 1, y: 0 },
          status: "normal",
          label: "b.ts",
          description: "",
          lastModified: "",
          agentPresent: undefined,
          kanbanCardId: undefined,
          untrackedChange: undefined,
          sins: [],
          notes: [],
        },
      ],
      agents: [
        {
          agentId: "orch-1",
          type: "orchestrator",
          status: "idle",
          currentFileId: null,
          currentTask: null,
          color: "#ff0000",
        },
      ],
      roads: [
        {
          roadId: "r1",
          from: "b1",
          to: "b2",
          type: "import" as const,
          style: "terra_battuta" as const,
          weight: 1,
        },
      ],
    });
    expect(citySignature(large).chars).toBeGreaterThan(citySignature(small).chars);
  });

  it("excluded keys are skipped at any depth (building.lastModified, top-level generatedAt)", () => {
    // Same structural content, different volatile fields on a NESTED building.
    const a = makeCity({
      generatedAt: "T1",
      buildings: [
        { ...makeCity().buildings[0], lastModified: "M1" },
      ],
    });
    const b = makeCity({
      generatedAt: "T2",
      buildings: [
        { ...makeCity().buildings[0], lastModified: "M2" },
      ],
    });
    const sa = citySignature(a);
    const sb = citySignature(b);
    expect(sa.sig).toBe(sb.sig);
    expect(sa.chars).toBe(sb.chars);
  });

  it("era changed => different sig (era is NOT excluded)", () => {
    const a = makeCity({ era: "Alpha" });
    const b = makeCity({ era: "Beta" });
    expect(citySignature(a).sig).not.toBe(citySignature(b).sig);
  });

  it("a string field change (building.label) => different sig", () => {
    // Pins that bare-string charCodes are actually folded into the hash — a
    // regression where the string branch returned without hashing would pass
    // every other test (which only mutate numbers / array lengths) but not this.
    const a = makeCity();
    const b = makeCity({
      buildings: [{ ...makeCity().buildings[0], label: "renamed.ts" }],
    });
    expect(citySignature(a).sig).not.toBe(citySignature(b).sig);
  });

  it("a nested sub-object change (building.coords) => different sig", () => {
    // A change two levels deep (building -> coords -> x) must still propagate.
    const a = makeCity();
    const b = makeCity({
      buildings: [{ ...makeCity().buildings[0], coords: { x: 99, y: 42 } }],
    });
    expect(citySignature(a).sig).not.toBe(citySignature(b).sig);
  });
});
