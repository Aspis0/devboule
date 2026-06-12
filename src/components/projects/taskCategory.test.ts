import { describe, it, expect } from "vitest";
import {
  TASK_CATEGORIES,
  categoryChipClass,
  categoryLabel,
  isTaskCategory,
} from "./taskCategory";

// Pure-logic tests for the Board card-category metadata. No DOM: the form and
// the TaskCard chip both consume these helpers, so the contract (the four
// allowed categories, the type guard, a stable label + chip class for each)
// is the single source of truth worth pinning.

describe("task category metadata", () => {
  it("exposes exactly the four locked categories", () => {
    expect([...TASK_CATEGORIES]).toEqual([
      "feature",
      "hardening",
      "bug",
      "other",
    ]);
  });

  it("type-guards only the known categories", () => {
    for (const category of TASK_CATEGORIES) {
      expect(isTaskCategory(category)).toBe(true);
    }
    expect(isTaskCategory("epic")).toBe(false);
    expect(isTaskCategory("")).toBe(false);
    expect(isTaskCategory(undefined)).toBe(false);
    expect(isTaskCategory(null)).toBe(false);
    expect(isTaskCategory(42)).toBe(false);
  });

  it("returns a non-empty label and chip class for every category", () => {
    for (const category of TASK_CATEGORIES) {
      expect(categoryLabel(category)).toMatch(/\S/);
      expect(categoryChipClass(category)).toMatch(/\S/);
    }
    expect(categoryLabel("bug")).toBe("Bug");
  });
});
