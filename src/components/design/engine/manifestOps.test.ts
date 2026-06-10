import { describe, it, expect } from "vitest";
import {
  moveNode,
  setPos,
  resizeNode,
  bringToFront,
  sendToBack,
  reorder,
} from "./manifestOps";
import type { DesignManifest, DesignNodePlacement } from "../../../types/design";

function placement(
  over: Partial<DesignNodePlacement> = {},
): DesignNodePlacement {
  return { x: 0, y: 0, z: 1, w: 100, h: "auto", kind: "html", ...over };
}

function manifest(
  nodes: Record<string, DesignNodePlacement>,
): DesignManifest {
  return { schemaVersion: 1, nodes };
}

describe("manifestOps — immutability", () => {
  it("moveNode does not mutate the input manifest or node", () => {
    const m = manifest({ a: placement({ x: 10, y: 20 }) });
    const before = JSON.stringify(m);
    const next = moveNode(m, "a", 5, -5);
    expect(JSON.stringify(m)).toBe(before); // input untouched
    expect(next).not.toBe(m); // new object
    expect(next.nodes).not.toBe(m.nodes);
    expect(next.nodes.a).not.toBe(m.nodes.a);
  });
});

describe("manifestOps — moveNode / setPos", () => {
  it("moveNode applies a relative delta", () => {
    const m = manifest({ a: placement({ x: 10, y: 20 }) });
    const next = moveNode(m, "a", 5, -3);
    expect(next.nodes.a.x).toBe(15);
    expect(next.nodes.a.y).toBe(17);
  });

  it("setPos sets an absolute position", () => {
    const m = manifest({ a: placement({ x: 10, y: 20 }) });
    const next = setPos(m, "a", 100, 200);
    expect(next.nodes.a.x).toBe(100);
    expect(next.nodes.a.y).toBe(200);
  });

  it("moveNode on a missing id returns the same manifest unchanged", () => {
    const m = manifest({ a: placement() });
    const next = moveNode(m, "ghost", 5, 5);
    expect(next).toBe(m);
  });

  it("setPos on a missing id returns the same manifest unchanged", () => {
    const m = manifest({ a: placement() });
    expect(setPos(m, "ghost", 1, 1)).toBe(m);
  });

  it("other nodes are preserved by identity (structural sharing)", () => {
    const m = manifest({ a: placement(), b: placement({ x: 50 }) });
    const next = moveNode(m, "a", 1, 1);
    expect(next.nodes.b).toBe(m.nodes.b); // untouched node shares identity
  });
});

describe("manifestOps — resizeNode", () => {
  it("sets w and a numeric h", () => {
    const m = manifest({ a: placement({ w: 100, h: "auto" }) });
    const next = resizeNode(m, "a", 250, 80);
    expect(next.nodes.a.w).toBe(250);
    expect(next.nodes.a.h).toBe(80);
  });

  it("keeps h auto when h is omitted", () => {
    const m = manifest({ a: placement({ w: 100, h: "auto" }) });
    const next = resizeNode(m, "a", 250);
    expect(next.nodes.a.w).toBe(250);
    expect(next.nodes.a.h).toBe("auto");
  });

  it("keeps an existing numeric h when h is omitted", () => {
    const m = manifest({ a: placement({ w: 100, h: 60 }) });
    const next = resizeNode(m, "a", 250);
    expect(next.nodes.a.h).toBe(60);
  });

  it("can explicitly pin h to auto", () => {
    const m = manifest({ a: placement({ w: 100, h: 60 }) });
    const next = resizeNode(m, "a", 250, "auto");
    expect(next.nodes.a.h).toBe("auto");
  });

  it("missing id returns unchanged manifest", () => {
    const m = manifest({ a: placement() });
    expect(resizeNode(m, "ghost", 10)).toBe(m);
  });

  it("does not mutate the input", () => {
    const m = manifest({ a: placement({ w: 100 }) });
    const before = JSON.stringify(m);
    resizeNode(m, "a", 999, 999);
    expect(JSON.stringify(m)).toBe(before);
  });
});

describe("manifestOps — bringToFront / sendToBack", () => {
  it("bringToFront sets z = max(z)+1 over all nodes", () => {
    const m = manifest({
      a: placement({ z: 1 }),
      b: placement({ z: 5 }),
      c: placement({ z: 3 }),
    });
    const next = bringToFront(m, "a");
    expect(next.nodes.a.z).toBe(6); // max was 5
    expect(next.nodes.b.z).toBe(5);
    expect(next.nodes.c.z).toBe(3);
  });

  it("sendToBack sets z = min(z)-1 over all nodes", () => {
    const m = manifest({
      a: placement({ z: 2 }),
      b: placement({ z: 5 }),
      c: placement({ z: 3 }),
    });
    const next = sendToBack(m, "b");
    expect(next.nodes.b.z).toBe(1); // min was 2
  });

  it("bringToFront on the only node yields z = its z + 1 (idempotent-ish, deterministic)", () => {
    const m = manifest({ a: placement({ z: 7 }) });
    const next = bringToFront(m, "a");
    expect(next.nodes.a.z).toBe(8);
  });

  it("bringToFront missing id returns unchanged", () => {
    const m = manifest({ a: placement({ z: 1 }) });
    expect(bringToFront(m, "ghost")).toBe(m);
  });

  it("does not mutate input", () => {
    const m = manifest({ a: placement({ z: 1 }), b: placement({ z: 2 }) });
    const before = JSON.stringify(m);
    bringToFront(m, "a");
    expect(JSON.stringify(m)).toBe(before);
  });
});

describe("manifestOps — reorder", () => {
  it("moves an id from one index to another (down)", () => {
    expect(reorder(["a", "b", "c", "d"], 0, 2)).toEqual(["b", "c", "a", "d"]);
  });

  it("moves an id up", () => {
    expect(reorder(["a", "b", "c", "d"], 3, 1)).toEqual(["a", "d", "b", "c"]);
  });

  it("no-op when from === to", () => {
    const order = ["a", "b", "c"];
    expect(reorder(order, 1, 1)).toEqual(["a", "b", "c"]);
  });

  it("does not mutate the input array", () => {
    const order = ["a", "b", "c"];
    const before = [...order];
    reorder(order, 0, 2);
    expect(order).toEqual(before);
  });

  it("clamps out-of-range indices instead of throwing", () => {
    expect(reorder(["a", "b", "c"], 0, 99)).toEqual(["b", "c", "a"]);
    expect(reorder(["a", "b", "c"], -5, 1)).toEqual(["b", "a", "c"]);
  });

  it("returns the same array reference for an empty list", () => {
    const empty: string[] = [];
    expect(reorder(empty, 0, 0)).toBe(empty);
  });
});
