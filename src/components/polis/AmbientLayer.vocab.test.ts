import { describe, it, expect } from "vitest";
import { AMBIENT_TYPES } from "./AmbientLayer";

// Polis-P2 — the idle DECORATIVE crowd vocabulary. It must contain a reference
// omino of every CLAIMABLE citizen type so a role's figure roams even before any
// real agent of that role activates. `merchant` stays EXCLUDED (reserved for the
// data-bound TradeRouteLayer porters).

describe("AMBIENT_TYPES — claimable-type idle vocabulary", () => {
  it("contains the 5 claimable citizen types", () => {
    for (const t of ["builder", "noble", "citizen", "watercarrier", "firefighter"] as const) {
      expect(AMBIENT_TYPES).toContain(t);
    }
  });

  it("EXCLUDES merchant (reserved for TradeRouteLayer porters)", () => {
    expect(AMBIENT_TYPES).not.toContain("merchant");
  });

  it("has exactly the 5 claimable types and no duplicates", () => {
    expect(AMBIENT_TYPES.length).toBe(5);
    expect(new Set(AMBIENT_TYPES).size).toBe(AMBIENT_TYPES.length);
  });

  it("is frozen so a consumer can't mutate the shared crowd vocabulary", () => {
    expect(Object.isFrozen(AMBIENT_TYPES)).toBe(true);
    // A mutation attempt must not change the array (throws in strict mode,
    // which test modules are, but assert the contents stay intact regardless).
    expect(() => {
      (AMBIENT_TYPES as unknown as string[]).push("merchant");
    }).toThrow();
    expect(AMBIENT_TYPES.length).toBe(5);
  });
});
