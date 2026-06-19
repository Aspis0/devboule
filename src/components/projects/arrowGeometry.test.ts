import { describe, it, expect } from "vitest";
import { buildArrowEdges, arrowheadPoints } from "./arrowGeometry";

describe("arrowGeometry", () => {
  describe("buildArrowEdges", () => {
    it("drops a dependency that is not in the present set", () => {
      const tasks = [{ id: "A", dependsOn: ["B"] }];
      expect(buildArrowEdges(tasks, new Set(["A"]))).toEqual([]);
    });

    it("drops a self-dependency", () => {
      const tasks = [{ id: "A", dependsOn: ["A"] }];
      expect(buildArrowEdges(tasks, new Set(["A"]))).toEqual([]);
    });

    it("deduplicates identical edges", () => {
      const tasks = [{ id: "B", dependsOn: ["A", "A"] }];
      expect(buildArrowEdges(tasks, new Set(["A", "B"]))).toEqual([{ from: "A", to: "B" }]);
    });

    it("handles undefined dependsOn", () => {
      const tasks = [{ id: "A" }];
      expect(buildArrowEdges(tasks, new Set(["A"]))).toEqual([]);
    });

    it("identifies a normal A -> B edge", () => {
      const tasks = [{ id: "A" }, { id: "B", dependsOn: ["A"] }];
      expect(buildArrowEdges(tasks, new Set(["A", "B"]))).toEqual([{ from: "A", to: "B" }]);
    });

    it("maintains stable order (task order, then dep order)", () => {
      const tasks = [
        { id: "B", dependsOn: ["A", "C"] },
        { id: "D", dependsOn: ["A"] },
      ];
      const result = buildArrowEdges(tasks, new Set(["A", "B", "C", "D"]));
      expect(result).toEqual([
        { from: "A", to: "B" },
        { from: "C", to: "B" },
        { from: "A", to: "D" },
      ]);
    });
  });

  describe("arrowheadPoints", () => {
    it("returns exactly 6 numbers", () => {
      const points = arrowheadPoints(10, 20, 0, 5);
      expect(points).toHaveLength(6);
      points.forEach((p) => expect(typeof p).toBe("number"));
    });

    it("sets the tip at the provided ex, ey", () => {
      const points = arrowheadPoints(100, 200, 0.5, 10);
      expect(points[0]).toBe(100);
      expect(points[1]).toBe(200);
    });

    it("places wings behind the tip when angleRad is 0", () => {
      const points = arrowheadPoints(100, 100, 0, 10);
      expect(points[2]).toBeLessThan(100); // leftX
      expect(points[4]).toBeLessThan(100); // rightX
    });

    it("wings are Y-symmetric about the tip for angleRad 0 (catches sin/cos sign error)", () => {
      const [, , lX, lY, rX, rY] = arrowheadPoints(100, 100, 0, 10);
      expect(lX).toBeCloseTo(rX); // same x at angle 0
      expect(lY - 100).toBeCloseTo(100 - rY); // symmetric around ey
    });

    it("places wings on the +x side when pointing left (angleRad = PI)", () => {
      const [, , lX, , rX] = arrowheadPoints(100, 100, Math.PI, 10);
      expect(lX).toBeGreaterThan(100);
      expect(rX).toBeGreaterThan(100);
    });
  });
});
