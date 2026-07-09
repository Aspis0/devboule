// Unit tests for the P3.2 pure filter precomputation.
import { describe, it, expect } from "vitest";
import type { CityState, FilterState, SinRecord, Building } from "../../types/city";
import { computeFilterSets } from "./filterModel";

function mkB(over: Partial<Building> & { fileId: string }): Building {
  return {
    filePath: `src/${over.fileId}.ts`,
    districtId: "d1",
    purpose: "house",
    purposeSource: "heuristic",
    linesOfCode: 100,
    visualTier: "oikia",
    coords: { x: 0, y: 0 },
    status: "normal",
    label: `${over.fileId}.ts`,
    description: "",
    lastModified: "",
    sins: [],
    notes: [],
    ...over,
  } as Building;
}

function mkRec(
  over: Partial<SinRecord> & { id: string; relPath: string },
): SinRecord {
  return {
    ruleId: "secret",
    line: null,
    severity: "smoke",
    description: "desc",
    evidence: "ev",
    disposition: "open",
    createdAt: "",
    updatedAt: "",
    fixDirectiveId: null,
    ...over,
  };
}

function defaultFilter(): FilterState {
  return { categories: [], minSeverity: null, features: [], pathGlob: "", mode: "ghost" };
}

describe("computeFilterSets", () => {
  it("empty filter = everything shown, nothing ghosted", () => {
    const city = { buildings: [mkB({ fileId: "a" }), mkB({ fileId: "b" })] } as CityState;
    const sets = computeFilterSets(city, [], defaultFilter());
    expect(sets.ghostedFileIds.size).toBe(0);
    expect(sets.effectsHiddenFileIds.size).toBe(0);
    expect(sets.shownBuildings).toBe(2);
    expect(sets.totalBuildings).toBe(2);
  });

  it("null city/records returns empty sets", () => {
    const sets = computeFilterSets(null, null, defaultFilter());
    expect(sets.ghostedFileIds.size).toBe(0);
    expect(sets.effectsHiddenFileIds.size).toBe(0);
    expect(sets.shownBuildings).toBe(0);
    expect(sets.totalBuildings).toBe(0);
    expect(sets.shownAnomalies).toBe(0);
    expect(sets.totalAnomalies).toBe(0);
  });

  describe("features axis", () => {
    it("keeps only buildings matching a feature id", () => {
      const city = {
        buildings: [
          mkB({ fileId: "a", featureId: "feat1" }),
          mkB({ fileId: "b", featureId: "feat2" }),
          mkB({ fileId: "c" }), // no featureId
        ],
      } as CityState;
      const f: FilterState = {
        ...defaultFilter(),
        features: ["feat1"],
      };
      const sets = computeFilterSets(city, [], f);
      expect(sets.ghostedFileIds.has("a")).toBe(false);
      expect(sets.ghostedFileIds.has("b")).toBe(true);
      expect(sets.ghostedFileIds.has("c")).toBe(true);
      expect(sets.shownBuildings).toBe(1);
    });

    it("empty features = all shown", () => {
      const city = {
        buildings: [mkB({ fileId: "a", featureId: "feat1" }), mkB({ fileId: "b" })],
      } as CityState;
      const sets = computeFilterSets(city, [], defaultFilter());
      expect(sets.ghostedFileIds.size).toBe(0);
    });
  });

  describe("pathGlob axis", () => {
    it("SRC/* matches src/foo.ts (case-insensitive + normalized)", () => {
      const city = {
        buildings: [
          mkB({ fileId: "a", filePath: "src/foo.ts" }),
          mkB({ fileId: "b", filePath: "test/bar.ts" }),
        ],
      } as CityState;
      const f: FilterState = { ...defaultFilter(), pathGlob: "SRC/*" };
      const sets = computeFilterSets(city, [], f);
      expect(sets.ghostedFileIds.has("a")).toBe(false);
      expect(sets.ghostedFileIds.has("b")).toBe(true);
    });

    it("*.ts does NOT match foo.tsx (anchored end)", () => {
      const city = {
        buildings: [
          mkB({ fileId: "a", filePath: "foo.ts" }),
          mkB({ fileId: "b", filePath: "foo.tsx" }),
        ],
      } as CityState;
      const f: FilterState = { ...defaultFilter(), pathGlob: "*.ts" };
      const sets = computeFilterSets(city, [], f);
      expect(sets.ghostedFileIds.has("a")).toBe(false);
      expect(sets.ghostedFileIds.has("b")).toBe(true);
    });

    it("dot-slash and backslash prefixes are normalized", () => {
      const city = {
        buildings: [
          mkB({ fileId: "a", filePath: "src/a.ts" }),
        ],
      } as CityState;
      let sets = computeFilterSets(city, [], { ...defaultFilter(), pathGlob: "./src/*" });
      expect(sets.ghostedFileIds.has("a")).toBe(false);

      sets = computeFilterSets(city, [], { ...defaultFilter(), pathGlob: "src\\*" });
      expect(sets.ghostedFileIds.has("a")).toBe(false);
    });

    it("substring fallback is case-insensitive", () => {
      const city = {
        buildings: [
          mkB({ fileId: "a", filePath: "src/components/POLIS/View.tsx" }),
          mkB({ fileId: "b", filePath: "src/utils/helper.ts" }),
        ],
      } as CityState;
      const f: FilterState = { ...defaultFilter(), pathGlob: "polis" };
      const sets = computeFilterSets(city, [], f);
      expect(sets.ghostedFileIds.has("a")).toBe(false);
      expect(sets.ghostedFileIds.has("b")).toBe(true);
    });

    it("empty glob = all shown", () => {
      const city = {
        buildings: [mkB({ fileId: "a" })],
      } as CityState;
      const sets = computeFilterSets(city, [], defaultFilter());
      expect(sets.ghostedFileIds.size).toBe(0);
    });
  });

  describe("categories axis", () => {
    it("hides effects when ALL open sins of a rule are filtered", () => {
      const city = {
        buildings: [mkB({ fileId: "a", filePath: "src/a.ts" })],
      } as CityState;
      const records = [mkRec({ id: "s1", relPath: "src/a.ts", ruleId: "secret" })];
      const f: FilterState = { ...defaultFilter(), categories: ["secret"] };
      const sets = computeFilterSets(city, records, f);
      // Building is NOT ghosted
      expect(sets.ghostedFileIds.has("a")).toBe(false);
      // Effects are hidden because the only sin is "secret" which is filtered
      expect(sets.effectsHiddenFileIds.has("a")).toBe(true);
      // Anomaly is not shown (effect hidden)
      expect(sets.shownAnomalies).toBe(0);
      expect(sets.totalAnomalies).toBe(1);
    });

    it("does NOT hide effects when some open sins are NOT filtered", () => {
      const city = {
        buildings: [mkB({ fileId: "a", filePath: "src/a.ts" })],
      } as CityState;
      const records = [
        mkRec({ id: "s1", relPath: "src/a.ts", ruleId: "secret" }),
        mkRec({ id: "s2", relPath: "src/a.ts", ruleId: "dead-export" }),
      ];
      const f: FilterState = { ...defaultFilter(), categories: ["secret"] };
      const sets = computeFilterSets(city, records, f);
      // "dead-export" is NOT filtered → effects stay visible
      expect(sets.effectsHiddenFileIds.has("a")).toBe(false);
      expect(sets.shownAnomalies).toBe(1); // dead-export shown
    });
  });

  describe("severity floor axis", () => {
    it("hides effects when ALL open sins are below the severity threshold", () => {
      const city = {
        buildings: [mkB({ fileId: "a", filePath: "src/a.ts" })],
      } as CityState;
      const records = [mkRec({ id: "s1", relPath: "src/a.ts", severity: "smoke" })];
      const f: FilterState = { ...defaultFilter(), minSeverity: "fire" };
      const sets = computeFilterSets(city, records, f);
      // smoke < fire → effects hidden
      expect(sets.effectsHiddenFileIds.has("a")).toBe(true);
      expect(sets.shownAnomalies).toBe(0);
    });

    it("keeps effects when some sins meet the severity floor", () => {
      const city = {
        buildings: [mkB({ fileId: "a", filePath: "src/a.ts" })],
      } as CityState;
      const records = [
        mkRec({ id: "s1", relPath: "src/a.ts", severity: "smoke" }),
        mkRec({ id: "s2", relPath: "src/a.ts", severity: "fire" }),
      ];
      const f: FilterState = { ...defaultFilter(), minSeverity: "fire" };
      const sets = computeFilterSets(city, records, f);
      // fire >= fire → effects NOT hidden
      expect(sets.effectsHiddenFileIds.has("a")).toBe(false);
      expect(sets.shownAnomalies).toBe(1); // fire shown, smoke hidden
    });

    it("null minSeverity = show all", () => {
      const records = [mkRec({ id: "s1", relPath: "src/a.ts", severity: "smoke" })];
      const city = {
        buildings: [mkB({ fileId: "a", filePath: "src/a.ts" })],
      } as CityState;
      const sets = computeFilterSets(city, records, defaultFilter());
      expect(sets.effectsHiddenFileIds.has("a")).toBe(false);
      expect(sets.shownAnomalies).toBe(1);
    });
  });

  describe("mode", () => {
    it("passes mode through to FilterSets", () => {
      const city = { buildings: [] } as unknown as CityState;
      const f: FilterState = { ...defaultFilter(), mode: "hide" };
      const sets = computeFilterSets(city, [], f);
      expect(sets.mode).toBe("hide");
    });
  });

  describe("combined axes", () => {
    it("features ghost + categories hide effects independently", () => {
      const city = {
        buildings: [
          mkB({ fileId: "a", featureId: "feat1", filePath: "src/a.ts" }),
          mkB({ fileId: "b", featureId: "feat2", filePath: "src/b.ts" }),
        ],
      } as CityState;
      const records = [
        mkRec({ id: "s1", relPath: "src/a.ts", ruleId: "secret" }),
        mkRec({ id: "s2", relPath: "src/b.ts", ruleId: "dead-export" }),
      ];
      const f: FilterState = {
        ...defaultFilter(),
        features: ["feat1"],
        categories: ["secret"],
      };
      const sets = computeFilterSets(city, records, f);
      // a: kept by features, but "secret" effect hidden
      expect(sets.ghostedFileIds.has("a")).toBe(false);
      expect(sets.effectsHiddenFileIds.has("a")).toBe(true);
      // b: ghosted (not feat1) — but effects hidden too because dead-export is filtered
      expect(sets.ghostedFileIds.has("b")).toBe(true);
      // effectsHidden only applies to non-ghosted buildings — but we still compute it
      // for ghosted ones (the renderer skips ghosted buildings' effects anyway)
      expect(sets.shownBuildings).toBe(1);
    });
  });

  describe("anomaly counts", () => {
    it("counts shown anomalies correctly", () => {
      const city = {
        buildings: [
          mkB({ fileId: "a", filePath: "src/a.ts" }),
          mkB({ fileId: "b", filePath: "src/b.ts" }),
        ],
      } as CityState;
      const records = [
        mkRec({ id: "s1", relPath: "src/a.ts", ruleId: "secret", severity: "fire" }),
        mkRec({ id: "s2", relPath: "src/b.ts", ruleId: "dead-export", severity: "smoke" }),
      ];
      // Filter: only hide secret effects, show fire+
      const f: FilterState = {
        ...defaultFilter(),
        categories: ["secret"],
        minSeverity: "fire",
      };
      const sets = computeFilterSets(city, records, f);
      // s1 (secret): hidden by category
      // s2 (smoke, dead-export): hidden by severity floor (smoke < fire)
      expect(sets.shownAnomalies).toBe(0);
      expect(sets.totalAnomalies).toBe(2);
    });
  });
});
