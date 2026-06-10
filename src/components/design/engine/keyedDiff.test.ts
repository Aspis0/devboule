import { describe, it, expect } from "vitest";
import {
  reanchorIds,
  structuralSignature,
  type ParsedNode,
} from "./keyedDiff";

// Helper to build a top-level parsed node concisely.
function node(
  tag: string,
  opts: {
    id?: string;
    attrs?: Record<string, string>;
    children?: ParsedNode[];
    text?: string;
  } = {},
): ParsedNode {
  return {
    tag,
    dataNodeId: opts.id,
    attrs: opts.attrs ?? {},
    children: opts.children ?? [],
    text: opts.text ?? "",
  };
}

describe("structuralSignature", () => {
  it("is stable for structurally identical trees regardless of data-node-id", () => {
    const a = node("section", { id: "x", children: [node("h1"), node("p")] });
    const b = node("section", { id: "y", children: [node("h1"), node("p")] });
    expect(structuralSignature(a)).toBe(structuralSignature(b));
  });

  it("differs when tag differs", () => {
    expect(structuralSignature(node("div"))).not.toBe(
      structuralSignature(node("span")),
    );
  });

  it("differs when child structure differs", () => {
    const a = node("section", { children: [node("h1")] });
    const b = node("section", { children: [node("h1"), node("p")] });
    expect(structuralSignature(a)).not.toBe(structuralSignature(b));
  });

  it("ignores the data-node-id attribute itself", () => {
    const a = node("div", { attrs: { "data-node-id": "a", class: "card" } });
    const b = node("div", { attrs: { "data-node-id": "b", class: "card" } });
    expect(structuralSignature(a)).toBe(structuralSignature(b));
  });
});

describe("reanchorIds — identity (no change)", () => {
  it("keeps ids when the new tree carries the exact same ids", () => {
    const prev = ["hero", "cta"];
    const next = [node("section", { id: "hero" }), node("button", { id: "cta" })];
    const result = reanchorIds(prev, next);
    expect(result.map((n) => n.dataNodeId)).toEqual(["hero", "cta"]);
  });
});

describe("reanchorIds — reorder", () => {
  it("preserves ids across a reorder (id-carried)", () => {
    const prev = ["hero", "cta"];
    const next = [node("button", { id: "cta" }), node("section", { id: "hero" })];
    const result = reanchorIds(prev, next);
    expect(result.map((n) => n.dataNodeId)).toEqual(["cta", "hero"]);
  });

  it("recovers ids by structure when ids are dropped after a reorder", () => {
    // Previous: hero=section, cta=button (in that order). New tree dropped ids
    // and reordered. Structure must re-anchor: the button -> cta, section -> hero.
    const prev = ["hero", "cta"];
    const prevShapes = {
      hero: node("section", { children: [node("h1")] }),
      cta: node("button"),
    };
    const next = [node("button"), node("section", { children: [node("h1")] })];
    const result = reanchorIds(prev, next, prevShapes);
    expect(result[0].dataNodeId).toBe("cta");
    expect(result[1].dataNodeId).toBe("hero");
  });
});

describe("reanchorIds — rename", () => {
  it("re-anchors a renamed id back to the previous stable id by position+structure", () => {
    // LLM renamed hero -> heroSection. We must reassign the OLD id "hero".
    const prev = ["hero", "cta"];
    const prevShapes = {
      hero: node("section", { children: [node("h1")] }),
      cta: node("button"),
    };
    const next = [
      node("section", { id: "heroSection", children: [node("h1")] }),
      node("button", { id: "cta" }),
    ];
    const result = reanchorIds(prev, next, prevShapes);
    expect(result[0].dataNodeId).toBe("hero"); // re-anchored, not heroSection
    expect(result[1].dataNodeId).toBe("cta");
  });
});

describe("reanchorIds — duplicate", () => {
  it("assigns the previous id to ONE element and fresh ids to duplicates", () => {
    const prev = ["hero"];
    const next = [node("section", { id: "hero" }), node("section", { id: "hero" })];
    const result = reanchorIds(prev, next);
    const ids = result.map((n) => n.dataNodeId);
    // Exactly one keeps "hero"; the duplicate gets a fresh, unique id.
    expect(ids.filter((id) => id === "hero").length).toBe(1);
    expect(new Set(ids).size).toBe(2); // all unique
    expect(ids.every((id) => typeof id === "string" && id!.length > 0)).toBe(true);
  });
});

describe("reanchorIds — insert", () => {
  it("keeps existing ids and mints a fresh id for a newly inserted node", () => {
    const prev = ["hero", "cta"];
    const next = [
      node("section", { id: "hero" }),
      node("div", {}), // brand new, no id
      node("button", { id: "cta" }),
    ];
    const result = reanchorIds(prev, next);
    expect(result[0].dataNodeId).toBe("hero");
    expect(result[2].dataNodeId).toBe("cta");
    const fresh = result[1].dataNodeId;
    expect(fresh).toBeTruthy();
    expect(["hero", "cta"]).not.toContain(fresh);
    expect(new Set(result.map((n) => n.dataNodeId)).size).toBe(3);
  });
});

describe("reanchorIds — delete", () => {
  it("simply drops a removed node; survivors keep their ids", () => {
    const prev = ["hero", "cta", "footer"];
    const next = [node("section", { id: "hero" }), node("footer", { id: "footer" })];
    const result = reanchorIds(prev, next);
    expect(result.map((n) => n.dataNodeId)).toEqual(["hero", "footer"]);
  });
});

describe("reanchorIds — minted ids never collide with a DROPPED prev id", () => {
  it("does not mint an id equal to a prev id the model dropped (WARNING 5)", () => {
    // prev = {n1, cta}. The model keeps `cta` but drops `n1` and adds a brand-new
    // node. The brand-new node must NOT be minted as `n1` (which would later make
    // the new node inherit the dropped node's placement in applyGeneration).
    const prev = ["n1", "cta"];
    const next = [
      node("button", { id: "cta" }),
      node("div", {}), // brand new, no id
    ];
    const result = reanchorIds(prev, next);
    const ids = result.map((n) => n.dataNodeId);
    expect(ids[0]).toBe("cta");
    // The minted id is unique AND not equal to ANY previous id (dropped or not).
    expect(prev).not.toContain(ids[1]);
    expect(new Set(ids).size).toBe(2);
  });
});

describe("reanchorIds — determinism & purity", () => {
  it("is deterministic: same inputs -> identical id assignment", () => {
    const prev = ["a", "b", "c"];
    const next = () => [node("div"), node("div"), node("div")];
    const r1 = reanchorIds(prev, next());
    const r2 = reanchorIds(prev, next());
    expect(r1.map((n) => n.dataNodeId)).toEqual(r2.map((n) => n.dataNodeId));
  });

  it("does not mutate the input nodes", () => {
    const next = [node("section", { id: "hero" })];
    const snapshot = JSON.stringify(next);
    reanchorIds(["hero"], next);
    expect(JSON.stringify(next)).toBe(snapshot);
  });

  it("produces unique ids even when previous set is empty", () => {
    const result = reanchorIds([], [node("div"), node("div")]);
    const ids = result.map((n) => n.dataNodeId);
    expect(new Set(ids).size).toBe(2);
    expect(ids.every((id) => !!id)).toBe(true);
  });

  it("minted ids satisfy the node-id charset (^[a-z0-9][a-z0-9_-]{0,63}$)", () => {
    const result = reanchorIds([], [node("div"), node("div")]);
    const re = /^[a-z0-9][a-z0-9_-]{0,63}$/;
    for (const n of result) {
      expect(re.test(n.dataNodeId!)).toBe(true);
    }
  });

  it("re-anchored survivor ids also satisfy the charset (trusts validated prev ids)", () => {
    const result = reanchorIds(["hero", "cta"], [
      node("section", { id: "hero" }),
      node("button", { id: "cta" }),
    ]);
    const re = /^[a-z0-9][a-z0-9_-]{0,63}$/;
    for (const n of result) expect(re.test(n.dataNodeId!)).toBe(true);
  });
});
