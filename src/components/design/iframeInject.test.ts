// @vitest-environment jsdom
//
// jsdom tests for the THIN DOM-injection layer. Correctness logic lives in the
// pure engine; here we verify style strings, the DOMParser->ParsedNode wrapper,
// and idempotent host reconciliation + stale removal.

import { describe, it, expect, beforeEach } from "vitest";
import {
  buildShellHtml,
  placementStyle,
  parseTopLevelNodes,
  injectNodes,
  applyPlacement,
  CANVAS_ROOT_ID,
  NODE_ID_ATTR,
} from "./iframeInject";
import type {
  DesignNodePlacement,
  DesignProject,
} from "../../types/design";

function placement(over: Partial<DesignNodePlacement> = {}): DesignNodePlacement {
  return { x: 10, y: 20, z: 1, w: 200, h: "auto", kind: "html", ...over };
}

function project(
  nodes: Record<string, DesignNodePlacement>,
  components: Record<string, string>,
): DesignProject {
  return {
    meta: {
      schemaVersion: 1,
      id: "p",
      name: "t",
      createdAt: "",
      updatedAt: "",
      canvas: { w: 1440, h: 1024, grid: 8 },
      nodeOrder: Object.keys(nodes),
    },
    manifest: { schemaVersion: 1, nodes },
    components,
  };
}

describe("buildShellHtml", () => {
  it("contains the canvas root and NO script tags", () => {
    const html = buildShellHtml();
    expect(html).toContain(`id="${CANVAS_ROOT_ID}"`);
    expect(html.toLowerCase()).not.toContain("<script");
  });
});

describe("placementStyle", () => {
  it("emits absolute left/top/z/width and omits height for auto", () => {
    const s = placementStyle(placement({ x: 5, y: 6, z: 3, w: 100, h: "auto" }));
    expect(s).toContain("position:absolute");
    expect(s).toContain("left:5px");
    expect(s).toContain("top:6px");
    expect(s).toContain("z-index:3");
    expect(s).toContain("width:100px");
    expect(s).not.toContain("height:");
  });

  it("emits height for a numeric h", () => {
    const s = placementStyle(placement({ h: 80 }));
    expect(s).toContain("height:80px");
  });
});

describe("parseTopLevelNodes", () => {
  it("parses top-level elements with their data-node-id and structure", () => {
    const nodes = parseTopLevelNodes(
      '<section data-node-id="hero"><h1>Hi</h1></section><button data-node-id="cta">Go</button>',
    );
    expect(nodes).toHaveLength(2);
    expect(nodes[0].tag).toBe("section");
    expect(nodes[0].dataNodeId).toBe("hero");
    expect(nodes[0].children[0].tag).toBe("h1");
    expect(nodes[1].tag).toBe("button");
    expect(nodes[1].dataNodeId).toBe("cta");
  });

  it("returns [] for empty markup", () => {
    expect(parseTopLevelNodes("")).toEqual([]);
  });
});

describe("injectNodes — idempotent reconciliation", () => {
  let doc: Document;
  let root: HTMLElement;

  beforeEach(() => {
    document.documentElement.innerHTML = buildShellHtml();
    doc = document;
    root = doc.getElementById(CANVAS_ROOT_ID) as HTMLElement;
  });

  it("creates one host per manifest node with sanitized content + placement", () => {
    const p = project(
      { hero: placement({ x: 1, y: 2, z: 5, w: 300, h: "auto" }) },
      { hero: "<h1>Title</h1><script>alert(1)</script>" },
    );
    injectNodes(doc, p);
    const hosts = root.querySelectorAll(`:scope > [${NODE_ID_ATTR}]`);
    expect(hosts).toHaveLength(1);
    const host = hosts[0] as HTMLElement;
    expect(host.getAttribute(NODE_ID_ATTR)).toBe("hero");
    expect(host.innerHTML.toLowerCase()).toContain("<h1>");
    expect(host.innerHTML.toLowerCase()).not.toContain("<script"); // sanitized
    expect(host.getAttribute("style")).toContain("left:1px");
    expect(host.getAttribute("style")).toContain("z-index:5");
  });

  it("is idempotent: re-running does not duplicate hosts and refreshes content", () => {
    const p1 = project({ hero: placement() }, { hero: "<p>v1</p>" });
    injectNodes(doc, p1);
    const p2 = project({ hero: placement({ x: 99 }) }, { hero: "<p>v2</p>" });
    injectNodes(doc, p2);
    const hosts = root.querySelectorAll(`:scope > [${NODE_ID_ATTR}]`);
    expect(hosts).toHaveLength(1);
    expect((hosts[0] as HTMLElement).innerHTML).toContain("v2");
    expect((hosts[0] as HTMLElement).getAttribute("style")).toContain("left:99px");
  });

  it("removes stale hosts no longer in the manifest", () => {
    injectNodes(
      doc,
      project(
        { hero: placement(), cta: placement() },
        { hero: "<p>h</p>", cta: "<p>c</p>" },
      ),
    );
    expect(root.querySelectorAll(`:scope > [${NODE_ID_ATTR}]`)).toHaveLength(2);
    // cta removed from the manifest.
    injectNodes(doc, project({ hero: placement() }, { hero: "<p>h</p>" }));
    const remaining = root.querySelectorAll(`:scope > [${NODE_ID_ATTR}]`);
    expect(remaining).toHaveLength(1);
    expect((remaining[0] as HTMLElement).getAttribute(NODE_ID_ATTR)).toBe("hero");
  });

  it("does nothing (no throw) when the canvas root is absent", () => {
    document.documentElement.innerHTML = "<head></head><body></body>";
    expect(() =>
      injectNodes(document, project({ hero: placement() }, { hero: "x" })),
    ).not.toThrow();
  });

  // Regression for the Canvas `loadedRef` removal: injection correctness must not
  // depend on any external "has loaded" latch. Calling before the root exists is a
  // safe no-op, and a later call (after the shell parses / iframe reloads) injects
  // correctly — mirroring what `reinject` now does, gated purely on the in-call
  // contentDocument + canvas-root null-check.
  it("is correct across a reload cycle without any external loaded flag", () => {
    // 1) Root not present yet (pre-load / mid-reload): no-op, no throw.
    document.documentElement.innerHTML = "<head></head><body></body>";
    const p = project({ hero: placement({ x: 7 }) }, { hero: "<p>v</p>" });
    expect(() => injectNodes(document, p)).not.toThrow();
    expect(document.querySelectorAll(`[${NODE_ID_ATTR}]`)).toHaveLength(0);

    // 2) Shell parses (root appears): the very next call injects with no prior
    //    "load" signal needed.
    document.documentElement.innerHTML = buildShellHtml();
    const root2 = document.getElementById(CANVAS_ROOT_ID) as HTMLElement;
    injectNodes(document, p);
    const hosts = root2.querySelectorAll(`:scope > [${NODE_ID_ATTR}]`);
    expect(hosts).toHaveLength(1);
    expect((hosts[0] as HTMLElement).getAttribute("style")).toContain("left:7px");
  });
});

describe("applyPlacement — cheap drag path", () => {
  it("updates only the style of an existing host without touching innerHTML", () => {
    document.documentElement.innerHTML = buildShellHtml();
    const root = document.getElementById(CANVAS_ROOT_ID) as HTMLElement;
    injectNodes(
      document,
      project({ hero: placement({ x: 0 }) }, { hero: "<p>keep</p>" }),
    );
    const host = root.querySelector(`[${NODE_ID_ATTR}="hero"]`) as HTMLElement;
    const before = host.innerHTML;
    applyPlacement(root, "hero", placement({ x: 250, y: 300 }));
    expect(host.getAttribute("style")).toContain("left:250px");
    expect(host.getAttribute("style")).toContain("top:300px");
    expect(host.innerHTML).toBe(before); // content untouched
  });

  it("no-op when the host is absent", () => {
    document.documentElement.innerHTML = buildShellHtml();
    const root = document.getElementById(CANVAS_ROOT_ID) as HTMLElement;
    expect(() => applyPlacement(root, "ghost", placement())).not.toThrow();
  });
});
