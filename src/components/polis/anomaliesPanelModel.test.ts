import { describe, it, expect } from "vitest";
import { buildAnomaliesPanelModel, formatAge } from "./anomaliesPanelModel";
import type { SinRecord } from "../../types/city";

// ---------------------------------------------------------------------------
// formatAge
// ---------------------------------------------------------------------------
describe("formatAge", () => {
  const NOW = 1_700_000_000_000; // fixed reference

  it("returns 'just now' for negative drift", () => {
    const future = new Date(NOW + 5000).toISOString();
    expect(formatAge(future, NOW)).toBe("just now");
  });

  it("formats seconds", () => {
    const t = new Date(NOW - 30_000).toISOString();
    expect(formatAge(t, NOW)).toBe("30s");
  });

  it("formats minutes", () => {
    const t = new Date(NOW - 300_000).toISOString(); // 5 min
    expect(formatAge(t, NOW)).toBe("5m");
  });

  it("formats hours", () => {
    const t = new Date(NOW - 7_200_000).toISOString(); // 2h
    expect(formatAge(t, NOW)).toBe("2h");
  });

  it("formats days", () => {
    const t = new Date(NOW - 259_200_000).toISOString(); // 3d
    expect(formatAge(t, NOW)).toBe("3d");
  });

  it("returns '—' for invalid/unparseable createdAt", () => {
    expect(formatAge("not-a-date", NOW)).toBe("—");
    expect(formatAge("", NOW)).toBe("—");
    expect(formatAge("undefined", NOW)).toBe("—");
  });
});

// ---------------------------------------------------------------------------
// buildAnomaliesPanelModel
// ---------------------------------------------------------------------------

function mkSin(over: Partial<SinRecord> = {}): SinRecord {
  return {
    id: "s1",
    relPath: "src/a.ts",
    ruleId: "R1",
    line: 10,
    severity: "smoke",
    description: "desc",
    evidence: "",
    disposition: "open",
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    fixDirectiveId: null,
    ...over,
  };
}

const NOW = new Date("2026-01-10T00:00:00Z").getTime(); // 9 days after epoch

function makeBuildingMap(entries: [string, string][]): Map<string, string> {
  return new Map(entries);
}

describe("buildAnomaliesPanelModel", () => {
  it("returns empty model for null records", () => {
    const m = buildAnomaliesPanelModel(null, new Map(), NOW);
    expect(m.open).toEqual([]);
    expect(m.ignored).toEqual([]);
    expect(m.openCount).toBe(0);
  });

  it("returns empty model for empty records", () => {
    const m = buildAnomaliesPanelModel([], new Map(), NOW);
    expect(m.open).toEqual([]);
    expect(m.openCount).toBe(0);
  });

  it("partitions open vs ignored, excludes fixed", () => {
    const records = [
      mkSin({ id: "open1", disposition: "open" }),
      mkSin({ id: "ign1", disposition: "ignored" }),
      mkSin({ id: "fix1", disposition: "fixed" }),
    ];
    const m = buildAnomaliesPanelModel(records, new Map(), NOW);
    expect(m.open).toHaveLength(1);
    expect(m.open[0].sin.id).toBe("open1");
    expect(m.ignored).toHaveLength(1);
    expect(m.ignored[0].sin.id).toBe("ign1");
    expect(m.openCount).toBe(1);
  });

  it("sorts by severity desc, then oldest first", () => {
    const records = [
      mkSin({
        id: "smoke-new",
        severity: "smoke",
        disposition: "open",
        createdAt: "2026-01-09T00:00:00Z",
      }),
      mkSin({
        id: "inferno-old",
        severity: "inferno",
        disposition: "open",
        createdAt: "2026-01-02T00:00:00Z",
      }),
      mkSin({
        id: "fire-mid",
        severity: "fire",
        disposition: "open",
        createdAt: "2026-01-05T00:00:00Z",
      }),
    ];
    const m = buildAnomaliesPanelModel(records, new Map(), NOW);
    expect(m.open.map((r) => r.sin.id)).toEqual([
      "inferno-old",
      "fire-mid",
      "smoke-new",
    ]);
  });

  it("attaches age labels", () => {
    const records = [
      mkSin({
        id: "s1",
        disposition: "open",
        createdAt: "2026-01-08T00:00:00Z", // 2d ago
      }),
    ];
    const m = buildAnomaliesPanelModel(records, new Map(), NOW);
    expect(m.open[0].age).toBe("2d");
  });

  it("maps relPath to fileId via the building map", () => {
    const bMap = makeBuildingMap([["src/a.ts", "file-A"]]);
    const records = [mkSin({ relPath: "src/a.ts", disposition: "open" })];
    const m = buildAnomaliesPanelModel(records, bMap, NOW);
    expect(m.open[0].fileId).toBe("file-A");
  });

  it("sets fileId to null when building is not in the map", () => {
    const records = [mkSin({ relPath: "src/unknown.ts", disposition: "open" })];
    const m = buildAnomaliesPanelModel(records, new Map(), NOW);
    expect(m.open[0].fileId).toBeNull();
  });

  it("normalizes paths (backslash, leading ./)", () => {
    const bMap = makeBuildingMap([["src/b.ts", "file-B"]]);
    const records = [
      mkSin({ relPath: ".\\src\\b.ts", disposition: "open" }),
    ];
    const m = buildAnomaliesPanelModel(records, bMap, NOW);
    expect(m.open[0].fileId).toBe("file-B");
  });

  it("sorts ignored tab too", () => {
    const records = [
      mkSin({
        id: "ign-smoke",
        severity: "smoke",
        disposition: "ignored",
        createdAt: "2026-01-09T00:00:00Z",
      }),
      mkSin({
        id: "ign-inferno",
        severity: "inferno",
        disposition: "ignored",
        createdAt: "2026-01-03T00:00:00Z",
      }),
    ];
    const m = buildAnomaliesPanelModel(records, new Map(), NOW);
    expect(m.ignored.map((r) => r.sin.id)).toEqual([
      "ign-inferno",
      "ign-smoke",
    ]);
  });
});
