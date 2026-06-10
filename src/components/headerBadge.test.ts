import { describe, it, expect } from "vitest";
import { combineBadgeCount } from "./headerBadge";

describe("combineBadgeCount", () => {
  it("sums risks and attention", () => {
    expect(combineBadgeCount(3, 2)).toBe(5);
  });

  it("returns 0 when both are 0", () => {
    expect(combineBadgeCount(0, 0)).toBe(0);
  });

  it("handles attention alone", () => {
    expect(combineBadgeCount(0, 4)).toBe(4);
  });

  it("clamps negative / non-finite inputs to 0", () => {
    expect(combineBadgeCount(-1, 2)).toBe(2);
    expect(combineBadgeCount(NaN, 3)).toBe(3);
    expect(combineBadgeCount(2, -5)).toBe(2);
  });
});
