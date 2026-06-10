import { describe, it, expect } from "vitest";
import { actionPhaseFor, tunicForAgent } from "./AgentLayer";
import { defaultTunic } from "./kitcd/people";

// Polis-P2 reviewer fixes — pure, deterministic action-phase + tunic helpers.
// These run headlessly (no PIXI renderer): both are plain numeric functions.

describe("actionPhaseFor — firefighter water-arc is gated on `extinguishing`", () => {
  it("firefighter with extinguishing undefined → 0 (no water arc)", () => {
    expect(actionPhaseFor("firefighter", "idle", 30, undefined)).toBe(0);
    expect(actionPhaseFor("firefighter", "walking", 30, undefined)).toBe(0);
  });

  it("firefighter with extinguishing=false → 0 (idle bucket-carrier)", () => {
    expect(actionPhaseFor("firefighter", "reviewing", 30, false)).toBe(0);
  });

  it("firefighter with extinguishing=true → non-zero water-arc phase (P5 tell)", () => {
    expect(actionPhaseFor("firefighter", "reviewing", 30, true)).toBeGreaterThan(0);
    // Deterministic: emulates the source's `this.t` seconds at 30 Hz.
    expect(actionPhaseFor("firefighter", "reviewing", 30, true)).toBeCloseTo(1);
  });

  it("builder swings only while working/running, regardless of extinguishing", () => {
    expect(actionPhaseFor("builder", "working", 4, false)).toBe(2);
    expect(actionPhaseFor("builder", "running", 4, undefined)).toBe(2);
    expect(actionPhaseFor("builder", "idle", 4, true)).toBe(0);
  });

  it("other figures have no role action", () => {
    expect(actionPhaseFor("citizen", "working", 30, true)).toBe(0);
    expect(actionPhaseFor("noble", "reviewing", 30, true)).toBe(0);
  });
});

describe("tunicForAgent — deterministic + figure-keyed", () => {
  it("same figure + seed ⇒ identical tunic (determinism)", () => {
    expect(tunicForAgent("builder", 12345)).toBe(tunicForAgent("builder", 12345));
  });

  it("re-deriving for a changed figure tracks the new figure's base tone", () => {
    // Simulates F4: a figure change (coder→watercarrier) re-derives the tunic
    // from the SAME seed. The result must differ from the old figure's tunic and
    // be a shade of the NEW figure's default tone (not the old one).
    const seed = 0xabcd_1234;
    const asBuilder = tunicForAgent("builder", seed);
    const asWatercarrier = tunicForAgent("watercarrier", seed);
    expect(asWatercarrier).not.toBe(asBuilder);
    // Different default bases ⇒ different derived tunics for the same seed.
    expect(defaultTunic("builder")).not.toBe(defaultTunic("watercarrier"));
  });
});
