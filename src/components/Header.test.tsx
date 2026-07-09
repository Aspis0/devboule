// @vitest-environment jsdom
//
// JUMP_TARGETS must no longer reference the cloud "Providers" area (S1). The
// view stays reachable by deep link, so this only asserts the search targets
// don't point at "providers" (in full or via a "providers#tab" deep link).

import { describe, it, expect } from "vitest";
import { JUMP_TARGETS } from "./Header";

describe("Header JUMP_TARGETS (S1)", () => {
  it("no jump target references the hidden providers view", () => {
    expect(JUMP_TARGETS.length).toBeGreaterThan(0);
    for (const entry of JUMP_TARGETS) {
      expect(entry.target).not.toBe("providers");
      expect(entry.target.startsWith("providers#")).toBe(false);
    }
  });
});
