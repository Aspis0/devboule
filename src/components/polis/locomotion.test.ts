import { describe, it, expect } from "vitest";
import {
  catmullRomPoint,
  buildSplineLeg,
  laneOffset,
  directedLaneOffset,
  applyPerpendicularOffset,
  SlotAllocator,
  type IPoint,
} from "./locomotion";

// ---- helpers ----
const pt = (x: number, y: number): IPoint => ({ x, y });
const dist = (a: IPoint, b: IPoint) => Math.hypot(a.x - b.x, a.y - b.y);

describe("catmullRomPoint", () => {
  it("t=0 returns p1 exactly", () => {
    const p0 = pt(0, 0);
    const p1 = pt(10, 20);
    const p2 = pt(30, 40);
    const p3 = pt(50, 60);
    const r = catmullRomPoint(p0, p1, p2, p3, 0);
    expect(r.x).toBeCloseTo(p1.x, 10);
    expect(r.y).toBeCloseTo(p1.y, 10);
  });

  it("t=1 returns p2 exactly", () => {
    const p0 = pt(0, 0);
    const p1 = pt(10, 20);
    const p2 = pt(30, 40);
    const p3 = pt(50, 60);
    const r = catmullRomPoint(p0, p1, p2, p3, 1);
    expect(r.x).toBeCloseTo(p2.x, 10);
    expect(r.y).toBeCloseTo(p2.y, 10);
  });

  it("t=0.5 is between p1 and p2 (smooth curve, not linear)", () => {
    const p0 = pt(0, 0);
    const p1 = pt(10, 20);
    const p2 = pt(30, 40);
    const p3 = pt(50, 60);
    const r = catmullRomPoint(p0, p1, p2, p3, 0.5);
    // Midpoint should be near (20, 30) but pulled toward p0/p3 influence
    expect(r.x).toBeGreaterThan(15);
    expect(r.x).toBeLessThan(25);
    expect(r.y).toBeGreaterThan(25);
    expect(r.y).toBeLessThan(35);
  });

  it("collinear points produce a straight line (degenerates to linear)", () => {
    const p0 = pt(0, 0);
    const p1 = pt(10, 0);
    const p2 = pt(20, 0);
    const p3 = pt(30, 0);
    for (let i = 0; i <= 10; i++) {
      const t = i / 10;
      const r = catmullRomPoint(p0, p1, p2, p3, t);
      expect(r.x).toBeCloseTo(10 + t * 10, 10);
      expect(r.y).toBeCloseTo(0, 10);
    }
  });
});

describe("buildSplineLeg", () => {
  it("straight 2-point route degrades to exact linear path", () => {
    const route = [pt(0, 0), pt(100, 0)];
    const sample = buildSplineLeg(route, 0);
    for (let i = 0; i <= 10; i++) {
      const t = i / 10;
      const r = sample(t);
      expect(r.x).toBeCloseTo(t * 100, 10);
      expect(r.y).toBeCloseTo(0, 10);
    }
  });

  it("straight 3-point colinear route endpoints are exact, mids near-linear", () => {
    const route = [pt(0, 0), pt(50, 0), pt(100, 0)];
    const s0 = buildSplineLeg(route, 0);
    const s1 = buildSplineLeg(route, 1);
    // Endpoints are exact (Catmull-Rom passes through p1 at t=0 and p2 at t=1).
    expect(s0(0).x).toBeCloseTo(0, 10);
    expect(s0(1).x).toBeCloseTo(50, 10);
    expect(s1(0).x).toBeCloseTo(50, 10);
    expect(s1(1).x).toBeCloseTo(100, 10);
    // Midpoints may bow slightly due to repeated-endpoint Catmull-Rom on 3 points.
    // They should stay near the straight line (within ~5px at scale 50).
    for (let i = 0; i <= 5; i++) {
      const t = (i + 1) / 6; // avoid t=0 and t=1 (exact)
      const r0 = s0(t);
      const r1 = s1(t);
      expect(Math.abs(r0.x - t * 50)).toBeLessThan(6);
      expect(Math.abs(r0.y)).toBeLessThan(6);
      expect(Math.abs(r1.x - (50 + t * 50))).toBeLessThan(6);
      expect(Math.abs(r1.y)).toBeLessThan(6);
    }
  });

  it("t=0 at leg start returns first waypoint of the leg", () => {
    const route = [pt(10, 0), pt(20, 30), pt(50, 60), pt(90, 40)];
    const s = buildSplineLeg(route, 1); // leg from (20,30) to (50,60)
    const r = s(0);
    expect(r.x).toBeCloseTo(20, 10);
    expect(r.y).toBeCloseTo(30, 10);
  });

  it("t=1 at leg end returns second waypoint", () => {
    const route = [pt(10, 0), pt(20, 30), pt(50, 60), pt(90, 40)];
    const s = buildSplineLeg(route, 1);
    const r = s(1);
    expect(r.x).toBeCloseTo(50, 10);
    expect(r.y).toBeCloseTo(60, 10);
  });

  it("C1 continuity: samplers on either side of a joint converge to same point", () => {
    const route = [pt(0, 0), pt(100, 80), pt(200, 40)];
    // End of leg 0 (t=1)
    const s0 = buildSplineLeg(route, 0);
    const end = s0(1);
    // Start of leg 1 (t=0)
    const s1 = buildSplineLeg(route, 1);
    const start = s1(0);
    // Both should be exactly the shared waypoint.
    expect(end.x).toBeCloseTo(100, 10);
    expect(end.y).toBeCloseTo(80, 10);
    expect(start.x).toBeCloseTo(100, 10);
    expect(start.y).toBeCloseTo(80, 10);
  });

  it("C1 continuity: small delta on either side of joint is smooth", () => {
    const route = [pt(0, 0), pt(100, 100), pt(200, 0)];
    const s0 = buildSplineLeg(route, 0);
    const s1 = buildSplineLeg(route, 1);
    // Just before joint
    const before = s0(0.99);
    // Just after joint
    const after = s1(0.01);
    // Both should be very close to the joint point (100,100).
    // They should not diverge wildly.
    expect(dist(before, pt(100, 100))).toBeLessThan(10);
    expect(dist(after, pt(100, 100))).toBeLessThan(10);
    // And they should be close to each other.
    expect(dist(before, after)).toBeLessThan(8);
  });
});

describe("laneOffset", () => {
  it("same walkerId → same offset", () => {
    expect(laneOffset("agent-a")).toBe(laneOffset("agent-a"));
  });

  it("different walkerIds likely produce different offsets", () => {
    const offsets = new Set<number>();
    for (let i = 0; i < 30; i++) {
      offsets.add(laneOffset(`walker-${i}`));
    }
    // Should span more than just 1 value.
    expect(offsets.size).toBeGreaterThanOrEqual(3);
  });

  it("offset is in [-4, 4]", () => {
    for (let i = 0; i < 100; i++) {
      const o = laneOffset(`test:${i}`);
      expect(o).toBeGreaterThanOrEqual(-4);
      expect(o).toBeLessThanOrEqual(4);
    }
  });
});

describe("directedLaneOffset", () => {
  it("positive dominant axis → same sign as laneOffset", () => {
    const raw = laneOffset("w1");
    expect(directedLaneOffset("w1", 1, 0)).toBe(raw);
    expect(directedLaneOffset("w1", 0.5, 0.3)).toBe(raw); // dx dominant, positive
  });

  it("negative dominant axis → flipped sign", () => {
    const raw = laneOffset("w1");
    expect(directedLaneOffset("w1", -1, 0)).toBe(-raw);
    expect(directedLaneOffset("w1", 0.2, -0.9)).toBe(-raw); // dy dominant, negative
  });

  it("zero vector → defaults to positive (raw sign preserved)", () => {
    const raw = laneOffset("w1");
    expect(directedLaneOffset("w1", 0, 0)).toBe(raw);
  });
});

describe("applyPerpendicularOffset", () => {
  it("positive offset on horizontal segment moves point up", () => {
    const r = applyPerpendicularOffset(pt(0, 0), 1, 0, 5);
    expect(r.x).toBeCloseTo(0, 10);
    expect(r.y).toBeCloseTo(5, 10); // perpendicular to (1,0) → (0, 1)
  });

  it("negative offset on horizontal segment moves point down", () => {
    const r = applyPerpendicularOffset(pt(0, 0), 1, 0, -3);
    expect(r.y).toBeCloseTo(-3, 10);
  });

  it("offset works on diagonal segment", () => {
    const r = applyPerpendicularOffset(pt(10, 10), 1, 1, 7);
    // Perpendicular to (1,1) is (-1,1) normalized.
    // pos + (-dy, dx)/|d| * offset = (10,10) + (-1,1)/√2 * 7
    const expectedX = 10 + (-1 / Math.SQRT2) * 7;
    const expectedY = 10 + (1 / Math.SQRT2) * 7;
    expect(r.x).toBeCloseTo(expectedX, 5);
    expect(r.y).toBeCloseTo(expectedY, 5);
  });
});

describe("SlotAllocator", () => {
  it("fills slots 0, 1, 2 in order", () => {
    const sa = new SlotAllocator();
    expect(sa.acquire("b1", "w1")).toBe(0);
    expect(sa.acquire("b1", "w2")).toBe(1);
    expect(sa.acquire("b1", "w3")).toBe(2);
  });

  it("4th arrival returns -1 (overflow)", () => {
    const sa = new SlotAllocator();
    sa.acquire("b1", "w1");
    sa.acquire("b1", "w2");
    sa.acquire("b1", "w3");
    expect(sa.acquire("b1", "w4")).toBe(-1);
  });

  it("release frees the slot for reuse", () => {
    const sa = new SlotAllocator();
    sa.acquire("b1", "w1");
    sa.acquire("b1", "w2");
    sa.release("b1", "w1");
    expect(sa.acquire("b1", "w3")).toBe(0); // reuses freed slot
  });

  it("double-acquire is idempotent", () => {
    const sa = new SlotAllocator();
    expect(sa.acquire("b1", "w1")).toBe(0);
    expect(sa.acquire("b1", "w1")).toBe(0); // same walker, same slot
    expect(sa.acquire("b1", "w2")).toBe(1); // next free
  });

  it("release of non-occupant is a no-op", () => {
    const sa = new SlotAllocator();
    sa.acquire("b1", "w1");
    sa.release("b1", "w99"); // not present
    expect(sa.acquire("b1", "w2")).toBe(1); // slot 1, 0 still occupied
  });

  it("sweep removes walker from all buildings", () => {
    const sa = new SlotAllocator();
    sa.acquire("b1", "wx");
    sa.acquire("b2", "wx");
    sa.sweep("wx");
    expect(sa.acquire("b1", "w-new")).toBe(0); // slot 0 now free
    expect(sa.acquire("b2", "w-new2")).toBe(0);
  });

  it("positionFor: slots extend BACKWARD from door along approach", () => {
    const sa = new SlotAllocator();
    const door = pt(100, 200);
    const dir = pt(1, 0); // road approaches from left, dir points rightward into building
    // Slots go BACKWARD (away from building threshold): door - slot*12*dir_norm
    const p0 = sa.positionFor(0, door, dir);
    const p1 = sa.positionFor(1, door, dir);
    const p2 = sa.positionFor(2, door, dir);
    expect(p0.x).toBeCloseTo(100, 10);   // slot 0 at door
    expect(p1.x).toBeCloseTo(88, 10);    // slot 1: door - 12
    expect(p2.x).toBeCloseTo(76, 10);    // slot 2: door - 24
  });

  it("positionFor overflow (-1) returns slot 2 position (backward from door)", () => {
    const sa = new SlotAllocator();
    const door = pt(0, 0);
    const dir = pt(1, 0);
    const pOverflow = sa.positionFor(-1, door, dir);
    const pSlot2 = sa.positionFor(2, door, dir);
    expect(pOverflow.x).toBeCloseTo(pSlot2.x, 10); // door - 24
    expect(pOverflow.y).toBeCloseTo(pSlot2.y, 10);
  });

  it("different buildings have independent slots", () => {
    const sa = new SlotAllocator();
    expect(sa.acquire("b1", "w1")).toBe(0);
    expect(sa.acquire("b2", "w1")).toBe(0); // same walker, different building → slot 0
  });

  // B1 regression — origin→destination slot lifecycle:
  //  1. Walker arrives at origin → acquires slot 0
  //  2. Walker departs origin → releases slot (freed)
  //  3. Walker arrives at destination → acquires slot 0 (reused)
  //  4. No leak: origin slot 0 is free for the NEXT walker
  it("full origin→destination slot lifecycle with no leak", () => {
    const sa = new SlotAllocator();
    const walkerId = "agent-1";
    const originId = "bld-origin";
    const destId = "bld-dest";

    // Agent arrives at origin building — acquires slot 0
    expect(sa.acquire(originId, walkerId)).toBe(0);

    // Agent departs origin → release (simulates advanceWalk arrival at dest)
    sa.release(originId, walkerId);

    // Agent arrives at destination — acquires slot 0
    expect(sa.acquire(destId, walkerId)).toBe(0);

    // Later, a DIFFERENT agent arrives at origin — must get slot 0 (no leak)
    expect(sa.acquire(originId, "agent-2")).toBe(0);

    // Destination still has slot 0 occupied by agent-1
    expect(sa.acquire(destId, "agent-3")).toBe(1);

    // Cleanup: agent-1 departs destination
    sa.release(destId, walkerId);
    // Now slot 0 is free at dest
    expect(sa.acquire(destId, "agent-4")).toBe(0);
  });
});
