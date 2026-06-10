import { describe, it, expect } from "vitest";
import { snapToGrid, smartGuides } from "./snap";
import type { NodeRect } from "../../../types/design";

function rect(over: Partial<NodeRect> = {}): NodeRect {
  return { id: "r", x: 0, y: 0, w: 100, h: 50, z: 1, ...over };
}

describe("snapToGrid", () => {
  it("snaps to the nearest multiple of grid", () => {
    expect(snapToGrid(0, 8)).toBe(0);
    expect(snapToGrid(3, 8)).toBe(0);
    expect(snapToGrid(4, 8)).toBe(8); // .5 rounds up
    expect(snapToGrid(5, 8)).toBe(8);
    expect(snapToGrid(11, 8)).toBe(8);
    expect(snapToGrid(12, 8)).toBe(16); // exactly halfway -> rounds up
    expect(snapToGrid(13, 8)).toBe(16);
  });

  it("handles negative values", () => {
    expect(snapToGrid(-3, 8)).toBe(-0); // rounds to 0
    expect(snapToGrid(-5, 8)).toBe(-8);
    expect(snapToGrid(-12, 8)).toBe(-8);
  });

  it("returns the value unchanged for a non-positive grid (snapping disabled)", () => {
    expect(snapToGrid(13, 0)).toBe(13);
    expect(snapToGrid(13, -1)).toBe(13);
  });

  it("is deterministic for the same inputs", () => {
    expect(snapToGrid(13, 8)).toBe(snapToGrid(13, 8));
  });
});

describe("smartGuides", () => {
  it("returns no guides and a zero delta when nothing aligns within threshold", () => {
    const moving = rect({ x: 200, y: 200, w: 100, h: 50 });
    const others = [rect({ id: "a", x: 0, y: 0, w: 100, h: 50 })];
    const result = smartGuides(moving, others, 5);
    expect(result.guides).toEqual([]);
    expect(result.dx).toBe(0);
    expect(result.dy).toBe(0);
  });

  it("snaps left edge to another node's left edge within threshold", () => {
    const moving = rect({ id: "m", x: 102, y: 300, w: 100, h: 50 });
    const others = [rect({ id: "a", x: 100, y: 0, w: 100, h: 50 })];
    const result = smartGuides(moving, others, 5);
    // moving.x 102 should snap to 100 -> dx = -2
    expect(result.dx).toBe(-2);
    const vertical = result.guides.find((g) => g.orientation === "vertical");
    expect(vertical?.position).toBe(100);
  });

  it("snaps horizontal-center to another node's horizontal center", () => {
    // moving center x = x + w/2. other center x = 150.
    const moving = rect({ id: "m", x: 98, y: 300, w: 100, h: 50 }); // center 148
    const others = [rect({ id: "a", x: 100, y: 0, w: 100, h: 50 })]; // center 150
    const result = smartGuides(moving, others, 5);
    expect(result.dx).toBe(2); // 148 -> 150
  });

  it("snaps top edge to another node's top edge", () => {
    const moving = rect({ id: "m", x: 500, y: 53, w: 100, h: 50 });
    const others = [rect({ id: "a", x: 0, y: 50, w: 100, h: 50 })];
    const result = smartGuides(moving, others, 5);
    expect(result.dy).toBe(-3);
    const horizontal = result.guides.find((g) => g.orientation === "horizontal");
    expect(horizontal?.position).toBe(50);
  });

  it("picks the closest candidate when multiple are within threshold", () => {
    const moving = rect({ id: "m", x: 103, y: 300, w: 100, h: 50 });
    const others = [
      rect({ id: "a", x: 100, y: 0, w: 100, h: 50 }), // left 100, dist 3
      rect({ id: "b", x: 101, y: 0, w: 100, h: 50 }), // left 101, dist 2 (closer)
    ];
    const result = smartGuides(moving, others, 5);
    expect(result.dx).toBe(-2); // snaps to the closer one (101)
  });

  it("ignores the moving node itself if present in others", () => {
    const moving = rect({ id: "m", x: 100, y: 100, w: 100, h: 50 });
    const others = [moving];
    const result = smartGuides(moving, others, 5);
    expect(result.guides).toEqual([]);
    expect(result.dx).toBe(0);
    expect(result.dy).toBe(0);
  });

  it("is pure: does not mutate the moving rect or others", () => {
    const moving = rect({ id: "m", x: 102, y: 53 });
    const others = [rect({ id: "a", x: 100, y: 50 })];
    const snapshot = JSON.stringify({ moving, others });
    smartGuides(moving, others, 5);
    expect(JSON.stringify({ moving, others })).toBe(snapshot);
  });
});
