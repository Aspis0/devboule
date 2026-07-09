// Unit tests for the Kin buildings pure model (P6.3).
//
// Pure (no DOM, no Tauri) — runs under the node-environment vitest config.

import { describe, it, expect } from "vitest";
import { topKin, kinBarWidth, type KinWire } from "./kinModel";

function mkKin(overrides: Partial<KinWire> & { relPath: string }): KinWire {
  return { score: 0.5, ...overrides };
}

describe("topKin", () => {
  it("returns empty array for empty input", () => {
    expect(topKin([])).toEqual([]);
  });

  it("returns all entries when fewer than 5", () => {
    const input = [
      mkKin({ relPath: "a.ts", score: 0.3 }),
      mkKin({ relPath: "b.ts", score: 0.7 }),
    ];
    const result = topKin(input);
    expect(result).toHaveLength(2);
  });

  it("caps at 5 entries", () => {
    const input = Array.from({ length: 8 }, (_, i) =>
      mkKin({ relPath: `f${i}.ts`, score: 0.1 * (i + 1) }),
    );
    expect(topKin(input)).toHaveLength(5);
  });

  it("sorts by score descending", () => {
    const input = [
      mkKin({ relPath: "low.ts", score: 0.2 }),
      mkKin({ relPath: "high.ts", score: 0.9 }),
      mkKin({ relPath: "mid.ts", score: 0.5 }),
    ];
    const result = topKin(input);
    expect(result.map((k) => k.relPath)).toEqual([
      "high.ts",
      "mid.ts",
      "low.ts",
    ]);
  });

  it("does not mutate the input array", () => {
    const a = mkKin({ relPath: "a.ts", score: 0.1 });
    const b = mkKin({ relPath: "b.ts", score: 0.9 });
    const input = [a, b];
    topKin(input);
    expect(input[0]).toBe(a);
    expect(input[1]).toBe(b);
  });
});

describe("kinBarWidth", () => {
  it("maps 0 to 0%", () => {
    expect(kinBarWidth(0)).toBe(0);
  });

  it("maps 1 to 100%", () => {
    expect(kinBarWidth(1)).toBe(100);
  });

  it("maps 0.5 to 50%", () => {
    expect(kinBarWidth(0.5)).toBe(50);
  });

  it("clamps negative scores to 0%", () => {
    expect(kinBarWidth(-0.3)).toBe(0);
  });

  it("clamps scores above 1 to 100%", () => {
    expect(kinBarWidth(1.5)).toBe(100);
  });
});
