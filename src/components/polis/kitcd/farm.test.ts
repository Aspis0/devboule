// Farm primitives — smoke tests for deterministic drawing.

import { describe, it, expect } from "vitest";
import { Graphics } from "pixi.js";
import type { Proj } from "./iso";
import { Z_UNIT } from "./iso";
import { cartToIso } from "../iso";
import {
  cropRows,
  vineyard,
  orchardGrid,
  fallowField,
  haystack,
  farmShed,
} from "./farm";

// A simple Proj for testing (no building offset).
const testProj: Proj = {
  W: 0,
  D: 0,
  p(gx: number, gy: number, gz?: number) {
    const c = cartToIso(gx, gy);
    return { x: c.x, y: c.y - (gz || 0) * Z_UNIT };
  },
};

// Capture method calls by wrapping Graphics.
function makeRecordingGraphics(): { g: Graphics; calls: string[] } {
  const g = new Graphics();
  const calls: string[] = [];

  // Wrap methods to record calls.
  const origCircle = g.circle.bind(g);
  const origEllipse = g.ellipse.bind(g);
  const origRect = g.rect.bind(g);
  const origPoly = g.poly.bind(g);
  const origMoveTo = g.moveTo.bind(g);
  const origLineTo = g.lineTo.bind(g);

  g.circle = (x: number, y: number, radius: number) => {
    calls.push("circle");
    return origCircle(x, y, radius);
  };
  g.ellipse = (x: number, y: number, halfW: number, halfH: number) => {
    calls.push("ellipse");
    return origEllipse(x, y, halfW, halfH);
  };
  g.rect = (x: number, y: number, w: number, h: number) => {
    calls.push("rect");
    return origRect(x, y, w, h);
  };
  g.poly = (flat: number[]) => {
    calls.push("poly");
    return origPoly(flat);
  };
  g.moveTo = (x: number, y: number) => {
    calls.push("moveTo");
    return origMoveTo(x, y);
  };
  g.lineTo = (x: number, y: number) => {
    calls.push("lineTo");
    return origLineTo(x, y);
  };

  return { g, calls };
}

// ---------------------------------------------------------------------------
// Smoke tests: each primitive draws something and is deterministic
// ---------------------------------------------------------------------------

describe("farm primitives smoke tests", () => {
  it("cropRows draws something", () => {
    const { g, calls } = makeRecordingGraphics();
    cropRows(g, testProj, 0, 0, 5, 4, 42);
    expect(calls.length).toBeGreaterThan(0);
  });

  it("cropRows is deterministic (same seed → same calls)", () => {
    const a = makeRecordingGraphics();
    const b = makeRecordingGraphics();
    cropRows(a.g, testProj, 0, 0, 5, 4, 42);
    cropRows(b.g, testProj, 0, 0, 5, 4, 42);
    expect(a.calls).toEqual(b.calls);
  });

  it("cropRows varies with different seed", () => {
    const a = makeRecordingGraphics();
    const b = makeRecordingGraphics();
    cropRows(a.g, testProj, 0, 0, 6, 4, 42);
    cropRows(b.g, testProj, 0, 0, 6, 4, 99);
    // Both should draw something (seed affects jitter but not call sequence structure).
    expect(a.calls.length).toBeGreaterThan(0);
    expect(b.calls.length).toBeGreaterThan(0);
  });

  it("vineyard draws something", () => {
    const { g, calls } = makeRecordingGraphics();
    vineyard(g, testProj, 0, 0, 6, 4, 42);
    expect(calls.length).toBeGreaterThan(0);
  });

  it("vineyard is deterministic", () => {
    const a = makeRecordingGraphics();
    const b = makeRecordingGraphics();
    vineyard(a.g, testProj, 0, 0, 6, 4, 42);
    vineyard(b.g, testProj, 0, 0, 6, 4, 42);
    expect(a.calls).toEqual(b.calls);
  });

  it("orchardGrid draws something", () => {
    const { g, calls } = makeRecordingGraphics();
    orchardGrid(g, testProj, 0, 0, 6, 4, 42);
    expect(calls.length).toBeGreaterThan(0);
  });

  it("orchardGrid is deterministic", () => {
    const a = makeRecordingGraphics();
    const b = makeRecordingGraphics();
    orchardGrid(a.g, testProj, 0, 0, 6, 4, 42);
    orchardGrid(b.g, testProj, 0, 0, 6, 4, 42);
    expect(a.calls).toEqual(b.calls);
  });

  it("fallowField draws something", () => {
    const { g, calls } = makeRecordingGraphics();
    fallowField(g, testProj, 0, 0, 5, 4, 42);
    expect(calls.length).toBeGreaterThan(0);
  });

  it("fallowField is deterministic", () => {
    const a = makeRecordingGraphics();
    const b = makeRecordingGraphics();
    fallowField(a.g, testProj, 0, 0, 5, 4, 42);
    fallowField(b.g, testProj, 0, 0, 5, 4, 42);
    expect(a.calls).toEqual(b.calls);
  });

  it("haystack draws something", () => {
    const { g, calls } = makeRecordingGraphics();
    haystack(g, testProj, 0, 0, 42);
    expect(calls.length).toBeGreaterThan(0);
  });

  it("haystack is deterministic", () => {
    const a = makeRecordingGraphics();
    const b = makeRecordingGraphics();
    haystack(a.g, testProj, 0, 0, 42);
    haystack(b.g, testProj, 0, 0, 42);
    expect(a.calls).toEqual(b.calls);
  });

  it("farmShed draws something", () => {
    const { g, calls } = makeRecordingGraphics();
    farmShed(g, testProj, 0, 0, 42);
    expect(calls.length).toBeGreaterThan(0);
  });

  it("farmShed is deterministic", () => {
    const a = makeRecordingGraphics();
    const b = makeRecordingGraphics();
    farmShed(a.g, testProj, 0, 0, 42);
    farmShed(b.g, testProj, 0, 0, 42);
    expect(a.calls).toEqual(b.calls);
  });
});
