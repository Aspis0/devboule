// @vitest-environment jsdom
//
// Reconciliation-matrix tests for the generation pipeline. Uses the REAL DOM
// parser + sanitize chokepoint (jsdom) so the data-flow is exercised end to end:
// extract -> parse -> reanchorIds -> placement -> sanitize.

import { describe, it, expect } from "vitest";
import {
  applyGeneration,
  applyEdit,
  applyNodeId,
  type ShapeMap,
} from "./pipeline";
import type { DesignProject } from "../../../types/design";

function emptyProject(grid = 8): DesignProject {
  return {
    meta: {
      schemaVersion: 1,
      id: "p",
      name: "t",
      createdAt: "1970-01-01T00:00:00Z",
      updatedAt: "1970-01-01T00:00:00Z",
      canvas: { w: 1440, h: 1024, grid },
      nodeOrder: [],
    },
    manifest: { schemaVersion: 1, nodes: {} },
    components: {},
  };
}

describe("applyNodeId", () => {
  it("replaces a model-written id with the resolved id", () => {
    expect(applyNodeId('<section data-node-id="wrong">x</section>', "hero")).toBe(
      '<section data-node-id="hero">x</section>',
    );
  });

  it("adds an id when the element carried none", () => {
    expect(applyNodeId("<button>Go</button>", "cta")).toBe(
      '<button data-node-id="cta">Go</button>',
    );
  });

  it("preserves other attributes", () => {
    const out = applyNodeId('<div class="card" style="color:red">x</div>', "n1");
    expect(out).toContain('class="card"');
    expect(out).toContain('style="color:red"');
    expect(out).toContain('data-node-id="n1"');
  });

  it("handles a void/self-closing tag", () => {
    // DOM serialization emits void elements without a trailing slash.
    expect(applyNodeId("<img/>", "n1")).toBe('<img data-node-id="n1">');
  });

  it("is robust to '>' inside an attribute value (no tag corruption)", () => {
    const out = applyNodeId('<div title="a>b">x</div>', "n1");
    expect(out).toContain('data-node-id="n1"');
    expect(out).toContain('title="a>b"'); // attribute preserved verbatim
    expect(out).toContain(">x</div>"); // single element, content intact
    // Re-parsing yields exactly ONE top-level element (tag not split by the '>').
    const reparsed = new DOMParser().parseFromString(out, "text/html");
    expect(reparsed.body.children).toHaveLength(1);
    expect(reparsed.body.children[0].getAttribute("data-node-id")).toBe("n1");
  });
});

describe("applyGeneration — fresh project (all minted)", () => {
  it("mints ids and gives deterministic default placement", () => {
    const text =
      '```html\n<section><h1>Hero</h1></section>\n<button>Go</button>\n```';
    const { project, newIds, shapes } = applyGeneration(emptyProject(), text);

    expect(newIds).toHaveLength(2);
    expect(project.meta.nodeOrder).toEqual(newIds);
    // All ids are charset-valid.
    const re = /^[a-z0-9][a-z0-9_-]{0,63}$/;
    for (const id of newIds) expect(re.test(id)).toBe(true);

    // Deterministic placement: first at margin, second one column over.
    const [a, b] = newIds;
    expect(project.manifest.nodes[a]).toMatchObject({ x: 40, y: 40, w: 360 });
    expect(project.manifest.nodes[b].x).toBe(40 + 360 + 40);
    expect(project.manifest.nodes[a].h).toBe("auto");

    // Sanitized markup carries the resolved id.
    expect(project.components[a]).toContain(`data-node-id="${a}"`);
    expect(shapes[a].tag).toBe("section");
    expect(shapes[b].tag).toBe("button");
  });

  it("is deterministic: identical input -> identical ids + placement", () => {
    const text = "<div>a</div><div>b</div>";
    const r1 = applyGeneration(emptyProject(), text);
    const r2 = applyGeneration(emptyProject(), text);
    expect(r1.project.meta.nodeOrder).toEqual(r2.project.meta.nodeOrder);
    expect(r1.project.manifest.nodes).toEqual(r2.project.manifest.nodes);
  });

  it("sets kind=svg for an svg root", () => {
    const { project, newIds } = applyGeneration(
      emptyProject(),
      '<svg viewBox="0 0 1 1"><rect/></svg>',
    );
    expect(project.manifest.nodes[newIds[0]].kind).toBe("svg");
  });
});

describe("applyGeneration — regeneration with survivors (placement kept)", () => {
  // A project with KNOWN, already-persisted ids (the realistic regen case: ids
  // were minted on the first generation and saved, so on disk they ARE "hero"/
  // "cta"). We construct it directly with stable ids + custom placement so the
  // survivor assertions reference real ids.
  function seed(): { project: DesignProject; shapes: ShapeMap } {
    const project: DesignProject = {
      ...emptyProject(),
      meta: { ...emptyProject().meta, nodeOrder: ["hero", "cta"] },
      manifest: {
        schemaVersion: 1,
        nodes: {
          hero: { x: 500, y: 300, z: 9, w: 420, h: "auto", kind: "html" },
          cta: { x: 80, y: 80, z: 2, w: 160, h: "auto", kind: "html" },
        },
      },
      components: {
        hero: '<section data-node-id="hero"><h1>Hero</h1></section>',
        cta: '<button data-node-id="cta">Go</button>',
      },
    };
    const shapes: ShapeMap = {
      hero: {
        tag: "section",
        dataNodeId: "hero",
        attrs: { "data-node-id": "hero" },
        children: [
          { tag: "h1", attrs: {}, children: [], text: "Hero" },
        ],
        text: "Hero",
      },
      cta: {
        tag: "button",
        dataNodeId: "cta",
        attrs: { "data-node-id": "cta" },
        children: [],
        text: "Go",
      },
    };
    return { project, shapes };
  }

  it("keeps survivor placement when ids are carried through", () => {
    const { project, shapes } = seed();
    const { project: next } = applyGeneration(
      project,
      '<section data-node-id="hero"><h1>Hero v2</h1></section><button data-node-id="cta">Go v2</button>',
      { prevShapes: shapes },
    );
    expect(next.manifest.nodes["hero"]).toMatchObject({ x: 500, y: 300, z: 9 });
    expect(next.components["hero"]).toContain("Hero v2");
  });

  it("re-anchors survivors structurally when ids are dropped/renamed", () => {
    const { project, shapes } = seed();
    // Model dropped ids entirely but kept the same structure/order.
    const { project: next, newIds } = applyGeneration(
      project,
      "<section><h1>Hero v3</h1></section><button>Go v3</button>",
      { prevShapes: shapes },
    );
    expect(newIds).toHaveLength(0); // both re-anchored, none minted
    expect(next.manifest.nodes["hero"]).toMatchObject({ x: 500, y: 300 });
    expect(next.components["hero"]).toContain("Hero v3");
  });

  it("adds a new node with default placement, survivors untouched", () => {
    const { project, shapes } = seed();
    const { project: next, newIds } = applyGeneration(
      project,
      '<section data-node-id="hero"><h1>Hero</h1></section><button data-node-id="cta">Go</button><footer>New</footer>',
      { prevShapes: shapes },
    );
    expect(newIds).toHaveLength(1);
    expect(next.manifest.nodes["hero"]).toMatchObject({ x: 500, y: 300 });
    // The new node sits below existing content (deterministic, snapped).
    const np = next.manifest.nodes[newIds[0]];
    expect(np.x % 8).toBe(0);
    expect(np.y % 8).toBe(0);
    expect(np.z).toBeGreaterThan(9);
  });

  it("places a new node below SURVIVORS only, not below dropped content (WARNING 6)", () => {
    // hero@y=80 survives; footer@y=2000 is dropped this regen; one brand-new node
    // is added. The new node must land just below hero (the only survivor), NOT
    // far below the dropped footer's y=2000 (which would push it off-screen).
    const project: DesignProject = {
      ...emptyProject(),
      meta: { ...emptyProject().meta, nodeOrder: ["hero", "footer"] },
      manifest: {
        schemaVersion: 1,
        nodes: {
          hero: { x: 80, y: 80, z: 1, w: 420, h: 200, kind: "html" },
          footer: { x: 80, y: 2000, z: 2, w: 420, h: 100, kind: "html" },
        },
      },
      components: {
        hero: '<section data-node-id="hero"><h1>Hero</h1></section>',
        footer: '<footer data-node-id="footer">F</footer>',
      },
    };
    const shapes: ShapeMap = {
      hero: {
        tag: "section",
        dataNodeId: "hero",
        attrs: { "data-node-id": "hero" },
        children: [{ tag: "h1", attrs: {}, children: [], text: "Hero" }],
        text: "Hero",
      },
      footer: {
        tag: "footer",
        dataNodeId: "footer",
        attrs: { "data-node-id": "footer" },
        children: [],
        text: "F",
      },
    };
    const { project: next, newIds } = applyGeneration(
      project,
      '<section data-node-id="hero"><h1>Hero</h1></section><aside>New</aside>',
      { prevShapes: shapes },
    );
    expect(newIds).toHaveLength(1);
    const np = next.manifest.nodes[newIds[0]];
    // hero bottom = 80 + 200 = 280; new node sits just below that, far above 2000.
    expect(np.y).toBeGreaterThanOrEqual(280);
    expect(np.y).toBeLessThan(600);
  });

  it("drops a removed node", () => {
    const { project, shapes } = seed();
    const { project: next } = applyGeneration(
      project,
      '<section data-node-id="hero"><h1>Hero</h1></section>',
      { prevShapes: shapes },
    );
    expect(Object.keys(next.manifest.nodes)).toEqual(["hero"]);
    expect(next.components["cta"]).toBeUndefined();
  });

  it("keeps placement across a reorder (id-carried)", () => {
    const { project, shapes } = seed();
    const { project: next } = applyGeneration(
      project,
      '<button data-node-id="cta">Go</button><section data-node-id="hero"><h1>Hero</h1></section>',
      { prevShapes: shapes },
    );
    expect(next.manifest.nodes["hero"]).toMatchObject({ x: 500, y: 300 });
    expect(next.meta.nodeOrder).toEqual(["cta", "hero"]);
  });
});

describe("applyEdit — single-node round-trip", () => {
  function seed(): { project: DesignProject } {
    const project: DesignProject = {
      ...emptyProject(),
      meta: { ...emptyProject().meta, nodeOrder: ["hero", "cta"] },
      manifest: {
        schemaVersion: 1,
        nodes: {
          hero: { x: 80, y: 80, z: 1, w: 420, h: "auto", kind: "html" },
          cta: { x: 80, y: 260, z: 2, w: 160, h: "auto", kind: "html" },
        },
      },
      components: {
        hero: '<section data-node-id="hero"><h1>Hero</h1></section>',
        cta: '<button data-node-id="cta">Go</button>',
      },
    };
    return { project };
  }

  it("swaps only the target node's markup, keeps id + placement", () => {
    const { project } = seed();
    const heroBefore = project.components["hero"];
    const ctaPlacement = project.manifest.nodes["cta"];

    const { project: next, changed } = applyEdit(
      project,
      "cta",
      '<button data-node-id="cta" style="background:#c2410c">Go now</button>',
    );

    expect(changed).toBe(true);
    expect(next.components["cta"]).toContain("Go now");
    expect(next.components["cta"]).toContain('data-node-id="cta"');
    // Placement untouched; other node byte-identical.
    expect(next.manifest.nodes["cta"]).toEqual(ctaPlacement);
    expect(next.components["hero"]).toBe(heroBefore);
  });

  it("re-anchors to the original id even when the model drops it", () => {
    const { project } = seed();
    const { project: next } = applyEdit(project, "cta", "<button>Go now</button>");
    expect(next.components["cta"]).toContain('data-node-id="cta"');
    expect(next.components["cta"]).toContain("Go now");
  });

  it("re-anchors to the original id even when the model renames it", () => {
    const { project } = seed();
    const { project: next } = applyEdit(
      project,
      "cta",
      '<button data-node-id="renamed">Go now</button>',
    );
    expect(next.components["cta"]).toContain('data-node-id="cta"');
    expect(next.components["cta"]).not.toContain("renamed");
  });

  it("refreshes kind to svg when the edited root tag changes html->svg (WARNING 7)", () => {
    const { project } = seed();
    expect(project.manifest.nodes["cta"].kind).toBe("html");
    const { project: next } = applyEdit(
      project,
      "cta",
      '<svg viewBox="0 0 1 1"><rect/></svg>',
    );
    expect(next.manifest.nodes["cta"].kind).toBe("svg");
    // Placement coordinates are otherwise untouched.
    expect(next.manifest.nodes["cta"]).toMatchObject({ x: 80, y: 260, z: 2 });
  });

  it("is a no-op for an unknown node id", () => {
    const { project } = seed();
    const r = applyEdit(project, "ghost", "<div>x</div>");
    expect(r.project).toBe(project);
    expect(r.changed).toBe(false);
  });

  it("sanitizes malicious markup in an edit", () => {
    const { project } = seed();
    const { project: next } = applyEdit(
      project,
      "cta",
      '<button data-node-id="cta">Go<img src=x onerror="alert(1)"></button>',
    );
    expect(next.components["cta"].toLowerCase()).not.toContain("onerror");
  });

  it("Tier 1: strips positional CSS from the edited root, keeps inner styles", () => {
    const { project } = seed();
    const { project: next } = applyEdit(
      project,
      "cta",
      '<button data-node-id="cta" style="position:absolute;top:10px;left:5px;background:#c2410c"><span style="position:absolute">!</span>Go</button>',
    );
    const m = next.components["cta"];
    // Root positional CSS gone; root brand color kept; inner position preserved.
    expect(m).not.toMatch(/<button[^>]*position/);
    expect(m).not.toContain("top:10px");
    expect(m).not.toContain("left:5px");
    expect(m).toContain("background:#c2410c");
    expect(m).toContain('<span style="position:absolute">');
  });

  it("Tier 1: keeps position:relative on the edited root (inner absolute child)", () => {
    const { project } = seed();
    const { project: next, changed } = applyEdit(
      project,
      "cta",
      '<button data-node-id="cta" style="position:relative;background:#c2410c"><span style="position:absolute;top:0">!</span>Go</button>',
    );
    expect(changed).toBe(true);
    const m = next.components["cta"];
    expect(m).toContain("position:relative");
    expect(m).toContain('<span style="position:absolute;top:0">');
  });

  it("WARNING 5: an edit returning a foster-parented root is a no-op (changed=false)", () => {
    const { project } = seed();
    const before = project.components["cta"];
    const { project: next, changed, warnings } = applyEdit(
      project,
      "cta",
      "<tr><td>oops</td></tr>",
    );
    expect(next).toBe(project); // unchanged reference: nothing swapped
    expect(changed).toBe(false);
    expect(warnings.length).toBeGreaterThan(0);
    expect(next.components["cta"]).toBe(before);
  });

  it("WARNING 5: an edit returning an empty root is a no-op (changed=false)", () => {
    const { project } = seed();
    const before = project.components["cta"];
    const { project: next, changed } = applyEdit(project, "cta", "just prose");
    expect(next).toBe(project);
    expect(changed).toBe(false);
    expect(next.components["cta"]).toBe(before);
  });

  it("WARNING 9: a multi-element edit keeps the first + warns", () => {
    const { project } = seed();
    // The real parse splits siblings into separate nodes; to exercise the per-node
    // MULTIPLE_TOP_LEVEL collapse on an EDIT we inject a node whose markup carries
    // two top-level elements.
    const fakeParse = () => [
      {
        node: {
          tag: "button",
          dataNodeId: undefined,
          attrs: {},
          children: [],
          text: "First",
        },
        markup: '<button data-node-id="cta">First</button><div>Second</div>',
      },
    ];
    const { project: next, changed, warnings } = applyEdit(
      project,
      "cta",
      "ignored",
      { parse: fakeParse },
    );
    expect(changed).toBe(true);
    expect(next.components["cta"]).toContain("First");
    expect(next.components["cta"]).not.toContain("Second");
    expect(warnings.length).toBeGreaterThan(0);
  });
});

describe("applyGeneration — Tier-1 contract guard (Phase 2.5)", () => {
  it("neutralizes positional CSS on the committed root, keeps inner styles", () => {
    const { project, newIds } = applyGeneration(
      emptyProject(),
      '<section style="position:absolute;top:10px;left:5px;margin:8px;background:#fff"><h1 style="position:absolute">Hi</h1></section>',
    );
    const m = project.components[newIds[0]];
    expect(m).not.toMatch(/<section[^>]*position/);
    expect(m).not.toContain("top:10px");
    expect(m).not.toContain("left:5px");
    expect(m).not.toMatch(/<section[^>]*margin/);
    expect(m).toContain("background:#fff");
    // Inner element styles intact.
    expect(m).toContain('<h1 style="position:absolute">');
  });

  it("safely DROPS a <tr>-rooted sibling (parser discards it); others survive", () => {
    // The HTML parser foster-parents <tr> OUT of the fragment entirely BEFORE the
    // pipeline guard runs, so it never reaches placement and no id is stamped on a
    // wrong element (Finding 9's safety invariant). The two valid siblings survive
    // intact. (A parser-discarded foster sibling produces no warning because the
    // content is gone before the per-node guard sees it — see the <option> case
    // below for a foster tag that DOES survive parsing and IS warned on.)
    const { project, newIds } = applyGeneration(
      emptyProject(),
      "<section><h1>Good</h1></section><tr><td>bad</td></tr><button>Go</button>",
    );
    expect(newIds).toHaveLength(2);
    expect(Object.keys(project.manifest.nodes)).toHaveLength(2);
    const tags = Object.values(project.components).map((m) =>
      m.slice(1, m.indexOf(">")).split(/[\s>]/)[0],
    );
    expect(tags).toContain("section");
    expect(tags).toContain("button");
    // The wrong-element id-stamp can never have happened: every stamped id is on a
    // real section/button root.
    for (const id of newIds) {
      expect(project.components[id]).toContain(`data-node-id="${id}"`);
    }
  });

  it("drops an <option>-rooted node WITH a warning (foster tag that survives parsing)", () => {
    // <option> is foster-parented contractually but the HTML parser KEEPS it as a
    // real element, so it reaches the per-node guard and is dropped + warned.
    const { project, newIds, warnings, remainingViolations } = applyGeneration(
      emptyProject(),
      "<section><h1>Good</h1></section><option>bad</option>",
    );
    expect(newIds).toHaveLength(1);
    expect(Object.keys(project.manifest.nodes)).toHaveLength(1);
    expect(warnings).toHaveLength(1);
    expect(warnings[0]).toMatch(/Dropped 1 node/);
    expect(remainingViolations.map((v) => v.code)).toEqual([
      "FOSTER_PARENTED_ROOT",
    ]);
  });

  it("clean generation reports no warnings", () => {
    const { warnings, remainingViolations } = applyGeneration(
      emptyProject(),
      "<section><h1>Hi</h1></section>",
    );
    expect(warnings).toEqual([]);
    expect(remainingViolations).toEqual([]);
  });

  it("dropping a foster node does not shift survivor placement off-screen", () => {
    // The dropped <tr> must not participate in placement at all.
    const { project, newIds } = applyGeneration(
      emptyProject(),
      "<tr><td>x</td></tr><section><h1>Real</h1></section>",
    );
    expect(newIds).toHaveLength(1);
    const p = project.manifest.nodes[newIds[0]];
    expect(p).toMatchObject({ x: 40, y: 40 });
  });

  it("WARNING 9: a node with multiple top-level elements commits the first + warns", () => {
    // parseTopLevelNodesWithMarkup yields ONE node per top-level element, so to
    // exercise the per-node MULTIPLE_TOP_LEVEL collapse we inject a parse fn that
    // returns a single node whose markup carries two siblings.
    const fakeParse = () => [
      {
        node: {
          tag: "div",
          dataNodeId: undefined,
          attrs: {},
          children: [],
          text: "a",
        },
        markup: "<div>a</div><section>b</section>",
      },
    ];
    const { project, newIds, warnings } = applyGeneration(
      emptyProject(),
      "ignored",
      { parse: fakeParse },
    );
    expect(newIds).toHaveLength(1);
    expect(project.components[newIds[0]]).toContain("a");
    expect(project.components[newIds[0]]).not.toContain("b");
    expect(warnings.some((w) => /one element per component/i.test(w))).toBe(true);
  });

  it("WARNING 7: caps processed nodes at 50 with one aggregate warning", () => {
    const text = Array.from({ length: 60 }, (_, i) => `<div>n${i}</div>`).join("");
    const { project, newIds, warnings } = applyGeneration(emptyProject(), text);
    expect(newIds).toHaveLength(50);
    expect(Object.keys(project.manifest.nodes)).toHaveLength(50);
    // One aggregate overflow warning naming the drop count (10).
    expect(
      warnings.some((w) => /processed the first 50/.test(w) && /dropped 10/.test(w)),
    ).toBe(true);
    // Warnings array itself is bounded.
    expect(warnings.length).toBeLessThanOrEqual(50);
  });
});
