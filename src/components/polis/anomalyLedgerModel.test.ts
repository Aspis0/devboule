// Unit tests for the Augure anomaly ledger pure model (P1.4 Part B).
//
// Pure (no DOM, no Tauri) — runs under the node-environment vitest config.

import { describe, it, expect } from "vitest";
import type { SinRecord } from "../../types/city";
import { buildAnomalyLedgerModel } from "./anomalyLedgerModel";

function mkSin(overrides: Partial<SinRecord> & { id: string }): SinRecord {
  return {
    relPath: "src/a.ts",
    ruleId: "R001",
    line: null,
    severity: "smoke",
    description: "desc",
    evidence: "ev",
    disposition: "open",
    createdAt: "",
    updatedAt: "",
    fixDirectiveId: null,
    ...overrides,
  };
}

describe("buildAnomalyLedgerModel", () => {
  it("returns empty buckets for empty input", () => {
    const m = buildAnomalyLedgerModel([], "src/a.ts");
    expect(m.open).toEqual([]);
    expect(m.ignored).toEqual([]);
    expect(m.fixedCount).toBe(0);
  });

  it("filters by relPath === filePath exactly", () => {
    const records = [
      mkSin({ id: "1", relPath: "src/a.ts", disposition: "open" }),
      mkSin({ id: "2", relPath: "src/b.ts", disposition: "open" }),
      mkSin({ id: "3", relPath: "src/a.ts", disposition: "ignored" }),
    ];
    const m = buildAnomalyLedgerModel(records, "src/a.ts");
    expect(m.open).toHaveLength(1);
    expect(m.open[0].id).toBe("1");
    expect(m.ignored).toHaveLength(1);
    expect(m.ignored[0].id).toBe("3");
  });

  it("sorts open records severity desc then ruleId asc", () => {
    const records = [
      mkSin({ id: "1", severity: "smoke", ruleId: "R002" }),
      mkSin({ id: "2", severity: "inferno", ruleId: "R001" }),
      mkSin({ id: "3", severity: "fire", ruleId: "R001" }),
      mkSin({ id: "4", severity: "inferno", ruleId: "R000" }),
    ];
    const m = buildAnomalyLedgerModel(records, "src/a.ts");
    expect(m.open.map((r) => r.id)).toEqual(["4", "2", "3", "1"]);
  });

  it("sorts ignored records the same way", () => {
    const records = [
      mkSin({ id: "1", severity: "smoke", ruleId: "R002", disposition: "ignored" }),
      mkSin({ id: "2", severity: "fire", ruleId: "R001", disposition: "ignored" }),
    ];
    const m = buildAnomalyLedgerModel(records, "src/a.ts");
    expect(m.ignored.map((r) => r.id)).toEqual(["2", "1"]);
  });

  it("counts fixed records but does not list them", () => {
    const records = [
      mkSin({ id: "1", disposition: "open" }),
      mkSin({ id: "2", disposition: "fixed" }),
      mkSin({ id: "3", disposition: "fixed" }),
      mkSin({ id: "4", disposition: "ignored" }),
    ];
    const m = buildAnomalyLedgerModel(records, "src/a.ts");
    expect(m.open).toHaveLength(1);
    expect(m.ignored).toHaveLength(1);
    expect(m.fixedCount).toBe(2);
  });

  it("unknown severity sorts last", () => {
    const records = [
      mkSin({ id: "1", severity: "unknown_level" as never, ruleId: "R001" }),
      mkSin({ id: "2", severity: "smoke", ruleId: "R001" }),
    ];
    const m = buildAnomalyLedgerModel(records, "src/a.ts");
    // smoke (rank 1) > unknown (rank 0), so smoke first
    expect(m.open.map((r) => r.id)).toEqual(["2", "1"]);
  });

  it("normalizes backslashes and leading ./ in relPath (M4)", () => {
    const records = [
      mkSin({ id: "1", relPath: ".\\src\\a.ts", disposition: "open" }),
      mkSin({ id: "2", relPath: "./src/a.ts", disposition: "open" }),
      mkSin({ id: "3", relPath: "src/b.ts", disposition: "open" }),
    ];
    const m = buildAnomalyLedgerModel(records, "src/a.ts");
    expect(m.open).toHaveLength(2);
    expect(m.open.map((r) => r.id)).toEqual(["1", "2"]);
  });
});
